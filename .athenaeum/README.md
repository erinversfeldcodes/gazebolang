# .athenaeum

How this repository describes itself to the agents that work on it, and which
agent skills it uses. The skills come from
[Athenaeum](https://github.com/erinversfeldcodes/athenaeum); the tool that reads
this directory is [Hephaestium](https://github.com/erinversfeldcodes/hephaestium)'s
`heph`.

| File | What it is |
|---|---|
| `manifest.toml` | The components of this repository, the commands that decide whether a change may proceed (gates), and where things are. A skill asks the manifest instead of assuming a path or a command. |
| `lock.toml` | The Athenaeum release this repository uses, by tag and by the commit that tag resolved to, and which skills it takes. |
| `skills/` | Copies of those skills, committed so a change to one is a diff that can be reviewed. |

`.claude/skills/<name>` links to the copy in `skills/`, which is where Claude Code
looks.

## Working with it

- `heph manifest check` says whether the manifest is valid.
- `heph gate run format`, `lint` or `test` runs a gate, and `--changed PATH` skips
  the ones a change does not touch.
- `heph sync --check` says whether the copies in `skills/` match the lock. It needs
  the network.
- `heph sync --update` moves the lock to the latest Athenaeum release and updates
  the copies.

**Do not edit `skills/` by hand.** `heph sync` replaces it, and a change to a skill
belongs in Athenaeum. `heph sync` also never overwrites a file in `.claude/skills/`
that it did not create, so a skill of this repository's own can sit beside these.
