# baucord for macOS — the app and its disk image

The desktop client on a Mac ships as **`baucord.app`**: one universal
bundle (Apple silicon and Intel) in a disk image,
`baucord-v<version>-macos.dmg`, which opens to the app and an
Applications shortcut to drag it onto. Being a real app, it opens its
window and nothing else: the bare `baucord` binary in the old zips
opened a Terminal window first, because that is what macOS does with a
command-line program.

| file | what | who uses it |
|---|---|---|
| `Info.plist` | the bundle's Info.plist; `package.sh` writes the version in | — |
| `baucord.entitlements` | what the hardened runtime allows: the microphone, nothing else | Developer ID signing |
| `package.sh` | binaries → `baucord.app` → the `.dmg` and the updater's `.zip`; signs and notarizes | `release.yml`, job `macos-app` |

## 1. What a release publishes for macOS

| asset | for |
|---|---|
| `baucord-v<v>-macos.dmg` | new installs: the website's macOS download |
| `baucord-v<v>-macos.zip` | the updater of a copy installed as `baucord.app` |
| `baucord-v<v>-macos-arm64.zip`, `-macos-intel.zip` | as before: the bare binaries and the server, for copies installed from them, whose updater still fetches these |

The updater tells the two kinds of install apart by where it runs. In
`…/baucord.app/Contents/MacOS/` it downloads the bundle zip into a
hidden `.baucord-update` folder beside the app, unpacks it there and
swaps the whole bundle in (two renames; on a failure the old app goes
back). A bundle is signed as one piece: replacing only its executable
would break the signature, and the permissions macOS ties to it —
microphone, screen recording, Accessibility — would be asked for again
after every update. Anywhere else it updates the bare binary, as it
always has.

**Where the app keeps its settings:** always
`~/Library/Application Support/baucord` (settings, `identity.key`, the
avatar, `baucord.log`). Never inside `baucord.app`, even when the user
could write there: the bundle is signed as one piece, and an update
replaces it whole, which would take anything kept in it along
(`client::config`, `inside_app_bundle`).

**Moving from a zip install** keeps nothing by itself: a zip install
keeps its settings beside its binary (a "portable" install), where the
app does not look. To bring the servers, nickname and — above all —
the identity (a server knows you by it: your roles, your DMs) across,
copy them before the app's first start:

```sh
mkdir -p ~/Library/Application\ Support/baucord
cd ~/path/to/the/old/zip/folder             # where the old baucord binary is
cp settings.json identity.key ~/Library/Application\ Support/baucord/
cp avatar.png ~/Library/Application\ Support/baucord/ 2>/dev/null || true
```

If the app was already started once, quit it and copy them over its
fresh ones.

## 2. Signing and notarizing (once, needs the paid Developer Program)

Without these secrets the workflow still builds the app and the disk
image, signed **ad hoc**: on macOS 15 and later, users then have to
open System Settings → Privacy & Security → **Open Anyway** the first
time. With them, the app and the disk image are signed with your
Developer ID and, on a release, **notarized** by Apple: they open like
any app from the web.

1. A **Developer ID Application** certificate. Only the Account Holder
   can make one: Xcode → Settings → Accounts → your team → Manage
   Certificates → **+** → *Developer ID Application*. (Not *Apple
   Distribution*: that one is for the App Store and TestFlight.)
2. Export it with its key: Keychain Access → login → My Certificates →
   *Developer ID Application: …* → right-click → Export → `.p12`, with
   a password.
3. Two repository secrets:
   ```sh
   R=mpwsh/baucord
   base64 < baucord-developer-id.p12 | tr -d '\n' | gh secret set MACOS_DEVELOPER_ID_P12_BASE64 -R $R
   gh secret set MACOS_DEVELOPER_ID_PASSWORD -R $R    # prompts: the .p12's password
   ```

Notarizing uses the App Store Connect API key already set up for
TestFlight (`ASC_API_KEY_P8_BASE64`, `ASC_API_KEY_ID`,
`ASC_API_ISSUER_ID`; ios/README.md §3c). It runs on releases only — it
waits on Apple, a few minutes per file — and a refusal fails the run
with Apple's log of why.

## 3. Trying a build

Every push to `main` runs the job too (signed if the secrets are there,
never notarized) and keeps the disk image for 7 days as the artifact
`macos-dmg-<commit>` of the *Build and Release* run. A copy from there
is not notarized: the first open needs **Open Anyway**, as above.

## 4. What to know

- **Where it may live:** updating writes beside the app, so it needs a
  folder the user can write: `/Applications` for an administrator, or
  `~/Applications`. Run from the disk image itself (read-only) or from
  a standard account's `/Applications`, the update fails with a message
  and the app stays as it is.
- **Ad hoc and updates:** with no Developer ID, macOS may ask the user
  to allow baucord to "modify apps" the first time it updates itself
  (System Settings → Privacy & Security → App Management). A Developer
  ID–signed app updating itself is not asked.
- **No App Store build:** the Mac App Store requires the sandbox, and
  the desktop's video (FFmpeg with x264, GPL-3.0) and keybinds
  (Accessibility) do not fit it as they are (PRODUCTION-PLAN.md, the
  2026-10-24 entry in §7).
- **Minimum:** macOS 11, as the binaries are built for.

## 5. Building it on your Mac

Two steps: `cargo build` makes the program, `package.sh` makes
`baucord.app` and the disk image from it. Passing `-` for the Intel
binary makes an Apple-silicon-only app — enough to try it (releases are
always universal):

```sh
brew install cmake opus pkg-config nasm ffmpeg@7
export FFMPEG_DIR="$(brew --prefix ffmpeg@7)"   # FFmpeg 7: ffmpeg-next 8 does not build against 8
export CMAKE_POLICY_VERSION_MINIMUM=3.5         # CMake 4 and libopus' old CMakeLists
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --locked --profile deploy \
  --target aarch64-apple-darwin -p desktop --features desktop/tray

VERSION="$(awk -F'"' '/^version = "/ { print $2; exit }' Cargo.toml)"   # the workspace's
bash macos/package.sh "$VERSION" target/aarch64-apple-darwin/deploy/baucord - dist
open "dist/baucord-v$VERSION-macos.dmg"
```

The disk image and the updater's zip land in `dist/` (any folder you
name last). For a universal one, pass an Intel `baucord` in place of
`-` — the one in a release's `baucord-v<v>-macos-intel.zip` will do.

- Leave it **signed ad hoc** (no `SIGN_IDENTITY`): a local build links
  Homebrew's FFmpeg dynamically, and the hardened runtime of a
  Developer ID signature refuses to load those libraries — the app
  would quit at launch. CI links FFmpeg statically and has no such
  problem.
- A build that already picked up FFmpeg 8 (errors about
  `AV_CODEC_ID_V410` or `supported_framerates`) keeps its bindings:
  `rm -rf target/aarch64-apple-darwin/deploy/build/ffmpeg-sys-next-*
  target/aarch64-apple-darwin/deploy/.fingerprint/ffmpeg-*` and build
  again.
- A disk image fetched with `gh run download`, or built here, carries no
  quarantine flag, so macOS does not warn on the first open as it would
  for a download. To see what users see:
  `xattr -w com.apple.quarantine "0081;$(printf %x "$(date +%s)");Safari;" file.dmg`.
- What cannot be tried locally: the update swap, which needs a newer
  release on baucord.com (DEVICE-CHECKLIST §4.4).
