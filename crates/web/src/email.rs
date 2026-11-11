#![cfg(feature = "ssr")]
// The Err(String) on each of these already is the full explanation; an
// `# Errors` doc section would just restate it.
#![allow(clippy::missing_errors_doc)]

/// Sends emails. Falls back to logging to stdout when `RESEND_API_KEY`
/// isn't set, so local dev and CI never need a real Resend account —
/// mirrors the hermetic-by-default pattern `thestacks` uses for its own
/// email tests.
#[derive(Clone)]
pub enum EmailSender {
    Resend {
        api_key: String,
        from: String,
        client: reqwest::Client,
        endpoint: String,
    },
    Console,
}

impl EmailSender {
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("RESEND_API_KEY") {
            Ok(api_key) if !api_key.is_empty() => Self::Resend {
                api_key,
                from: std::env::var("EMAIL_FROM")
                    .unwrap_or_else(|_| "onboarding@resend.dev".to_string()),
                client: reqwest::Client::new(),
                endpoint: "https://api.resend.com/emails".to_string(),
            },
            _ => Self::Console,
        }
    }

    #[cfg(test)]
    fn test_resend(endpoint: String) -> Self {
        Self::Resend {
            api_key: "test-key".to_string(),
            from: "test@example.com".to_string(),
            client: reqwest::Client::new(),
            endpoint,
        }
    }

    pub async fn send_password_reset(&self, to: &str, link: &str) -> Result<(), String> {
        self.send(to, "Reset your Gazebo password", &password_reset_html(link))
            .await
    }

    pub async fn send_invite(&self, to: &str, link: &str) -> Result<(), String> {
        self.send(to, "You're invited to Gazebo", &invite_html(link))
            .await
    }

    /// Sends a clearly-labeled, harmless email used only to prove the
    /// Resend integration actually works end to end — see
    /// `/internal/email-check`, which CI hits right after every deploy.
    ///
    /// Unlike `send_password_reset`, Console mode is a *failure* here, not
    /// a fallback: the whole point of this call is to prove real delivery
    /// works, so silently logging instead would make the deploy check
    /// pass even when `RESEND_API_KEY` is missing in an environment that's
    /// supposed to send real email.
    pub async fn send_test_email(&self, to: &str) -> Result<(), String> {
        if matches!(self, Self::Console) {
            return Err("RESEND_API_KEY is not set — still in Console mode".to_string());
        }
        self.send(to, "Gazebo deployment check", TEST_EMAIL_HTML)
            .await
    }

    async fn send(&self, to: &str, subject: &str, html: &str) -> Result<(), String> {
        match self {
            Self::Console => {
                log::info!("would send \"{subject}\" to {to}: {html}");
                Ok(())
            }
            Self::Resend {
                api_key,
                from,
                client,
                endpoint,
            } => {
                let response = client
                    .post(endpoint)
                    .bearer_auth(api_key)
                    .json(&serde_json::json!({
                        "from": from,
                        "to": to,
                        "subject": subject,
                        "html": html,
                    }))
                    .send()
                    .await
                    .inspect_err(|e| {
                        log::error!("email: Resend request for {subject:?} to {to} failed: {e}");
                    })
                    .map_err(|e| e.to_string())?;

                if response.status().is_success() {
                    Ok(())
                } else {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    log::error!(
                        "email: Resend returned {status} sending {subject:?} to {to}: {body}"
                    );
                    Err(format!("Resend returned {status}: {body}"))
                }
            }
        }
    }
}

const TEST_EMAIL_HTML: &str = "<p>Automated deployment check for Gazebo — nothing to do here.</p>";

fn password_reset_html(link: &str) -> String {
    format!(
        "<p>Click to reset your Gazebo password:</p><p><a href=\"{link}\">{link}</a></p>\
         <p>This link expires in 24 hours. If you didn't request this, you can ignore it.</p>"
    )
}

fn invite_html(link: &str) -> String {
    format!(
        "<p>You've been invited to Gazebo. Click to sign up:</p><p><a href=\"{link}\">{link}</a></p>\
         <p>This link expires in 7 days.</p>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn password_reset_email_body_snapshot() {
        insta::assert_snapshot!(password_reset_html(
            "https://gazebo.fly.dev/reset-password?token=abc123"
        ));
    }

    #[test]
    fn test_email_body_snapshot() {
        insta::assert_snapshot!(TEST_EMAIL_HTML);
    }

    #[test]
    fn invite_email_body_snapshot() {
        insta::assert_snapshot!(invite_html("https://gazebo.fly.dev/signup?token=abc123"));
    }

    #[tokio::test]
    async fn console_send_password_reset_always_succeeds() {
        let result = EmailSender::Console
            .send_password_reset(
                "someone@example.com",
                "https://example.com/reset-password?token=abc",
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_send_invite_always_succeeds() {
        let result = EmailSender::Console
            .send_invite(
                "someone@example.com",
                "https://example.com/signup?token=abc",
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_send_test_email_fails_since_it_proves_nothing() {
        let result = EmailSender::Console
            .send_test_email("someone@example.com")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resend_send_password_reset_hits_the_configured_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let sender = EmailSender::test_resend(format!("{}/emails", server.uri()));
        let result = sender
            .send_password_reset(
                "someone@example.com",
                "https://example.com/reset-password?token=abc",
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn resend_failure_response_surfaces_as_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails"))
            .respond_with(ResponseTemplate::new(422).set_body_string("invalid from address"))
            .mount(&server)
            .await;

        let sender = EmailSender::test_resend(format!("{}/emails", server.uri()));
        let result = sender.send_test_email("someone@example.com").await;
        let err = result.expect_err("a 422 from Resend should surface as Err");
        assert!(err.contains("422"));
        assert!(err.contains("invalid from address"));
    }
}
