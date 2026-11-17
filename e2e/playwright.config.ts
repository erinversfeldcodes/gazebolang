import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  workers: process.env.CI ? 2 : 4,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: process.env.BASE_URL ?? "http://127.0.0.1:3000",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "setup",
      testMatch: /.*\.setup\.ts/,
    },
    {
      // Production-shaped data (via the importer's own `create` stage) that
      // specs can rely on — see dictionary.seed.ts for why it exists.
      name: "seed",
      testMatch: /.*\.seed\.ts/,
      use: { storageState: ".auth/user.json" },
      dependencies: ["setup"],
    },
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        storageState: ".auth/user.json",
        // Lets media-cards.spec.ts actually exercise RecordButton's
        // getUserMedia/MediaRecorder path end to end: --use-fake-ui-for-
        // media-stream auto-accepts the permission prompt instead of
        // blocking on it, and --use-fake-device-for-media-stream supplies
        // a synthetic video pattern + audio tone in place of a real
        // mic/camera, so MediaRecorder gets genuine (if synthetic) data
        // to work with. `permissions` is Playwright's own belt-and-
        // suspenders grant for the same two devices.
        permissions: ["camera", "microphone"],
        launchOptions: {
          args: [
            "--use-fake-ui-for-media-stream",
            "--use-fake-device-for-media-stream",
          ],
        },
      },
      dependencies: ["setup", "seed"],
    },
  ],
});
