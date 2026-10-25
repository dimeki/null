# Releasing a new version

## 1. Make the release files

1. Change `version` in `Cargo.toml` (for example `1.0.0` to `1.0.1`). Numbers only,
   in `major.minor.patch` form. The app compares them as numbers, so `1.10.0` is newer than `1.9.0`.
2. Run:

```
powershell -ExecutionPolicy Bypass -File package\build-package.ps1
```

It runs the tests, builds the program, and writes into `dist\`:

- `ytmusic-lite-<version>-win64.zip`: the whole app in one folder. Unzip anywhere and run it.
- `ytmusic-lite-setup-<version>.exe`: a proper installer. Made only if
  [Inno Setup](https://jrsoftware.org/isinfo.php) is installed on the PC doing the build.

It also prints each file's SHA-256, which you can show next to the download so people can check it.

## 2. Publish a GitHub release

The app checks https://github.com/dimeki/null for its newest release, so publishing one is
all it takes. The tag must be the version in plain numbers with an optional leading `v`
(`v1.0.1`); drafts and pre-releases are ignored. The first line of the notes is shown in the banner.

```
gh release create v1.0.1 dist\ytmusic-lite-setup-1.0.1.exe dist\ytmusic-lite-1.0.1-win64.zip --notes "What changed"
```

Installed apps notice within about 24 hours. Get update opens the release page in the browser.
The repository must stay public (GitHub allows 60 unauthenticated checks an hour per address).
Keep the big files (libmpv, deno) out of git itself: GitHub rejects files over 100 MB there,
but release attachments are fine.

## Alternative: your own manifest on the relay

The app can instead read a small file from your relay (change `UPDATE_MANIFEST_URL` in
`src\config.rs` to `https://relay.dimeki.cc/latest.json`). Edit `latest.json` (the file the relay serves at `/latest.json`):

```json
{
  "version": "1.0.1",
  "url": "https://where-people-download-it",
  "notes": "One or two sentences about what changed."
}
```

No restart needed. Each app checks when it starts and once a day, so people see the
notice within about 24 hours. `url` must start with `https://`: the app just opens it
in their browser. It never downloads or runs anything by itself.

The relay needs permission to read that file: start it with `--allow-read=latest.json`
(already in the Dockerfile and the service file). With Docker, mount the file, for example
`-v /path/to/latest.json:/app/latest.json:ro`. `relay\latest.json.example` shows the format.

## Things to know

- **Windows warns about unknown publishers.** Without a code-signing certificate (it costs money),
  Windows SmartScreen shows "Windows protected your PC" for the installer and the .exe. Clicking
  "More info" then "Run anyway" works, but it puts some people off.
- **People update themselves.** Upgrading is: download the new installer and run it (it replaces
  the old version and keeps their queue and sign-in). With the zip, unzip over the old folder.
- **yt-dlp updates separately.** The Update button in Settings handles it, because YouTube breaks
  it more often than you'll release a new version.
- **Check the licences** in `THIRD-PARTY-NOTICES.txt` before sharing publicly.
