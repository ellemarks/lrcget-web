# LRCGET

Utility for mass-downloading LRC synced lyrics for your offline music library.

LRCGET will scan every files in your chosen directory for music files, then and try to download lyrics to a LRC files having the same name and save them to the same directory as your music files.

LRCGET is the official client of [LRCLIB](https://lrclib.net) service.

## Download

🎉 Latest version: v2.2.0

Visit the [release page](https://github.com/tranxuanthang/lrcget/releases) to download.

### Windows

EXE installer (recommended): [LRCGET_2.2.0_x64-setup.exe](https://github.com/tranxuanthang/lrcget/releases/download/2.2.0/LRCGET_2.2.0_x64-setup.exe)

MSI installer: [LRCGET_2.2.0_x64_en-US.msi](https://github.com/tranxuanthang/lrcget/releases/download/2.2.0/LRCGET_2.2.0_x64_en-US.msi)

### Linux

Flatpak build (recommended, for most Linux distros):

<a href='https://flathub.org/en/apps/net.lrclib.lrcget'><img width='120' alt='Get LRCGET on Flathub' src='https://flathub.org/api/badge?locale=en'/></a>

Deb packages (for Ubuntu 24.04+ and Linux Mint 22+): [LRCGET_2.2.0_amd64.deb](https://github.com/tranxuanthang/lrcget/releases/download/2.2.0/LRCGET_2.2.0_amd64.deb)

RPM packages (for Fedora, openSUSE, etc.): [LRCGET-2.2.0-1.x86_64.rpm](https://github.com/tranxuanthang/lrcget/releases/download/2.2.0/LRCGET-2.2.0-1.x86_64.rpm)

AppImage (for most Linux distros): [LRCGET_2.2.0_amd64.AppImage](https://github.com/tranxuanthang/lrcget/releases/download/2.2.0/LRCGET_2.2.0_amd64.AppImage)

### macOS

Mac x64 (Intel): [LRCGET_2.2.0_x64.dmg](https://github.com/tranxuanthang/lrcget/releases/download/2.2.0/LRCGET_2.2.0_x64.dmg)

Mac Apple Silicon: [LRCGET_2.2.0_aarch64.dmg](https://github.com/tranxuanthang/lrcget/releases/download/2.2.0/LRCGET_2.2.0_aarch64.dmg)

## Self-hosting the web version

LRCGET can also run as a web server, for example on a NAS next to your music, and be used from any browser. The same app is served over HTTP: the library, scanning, downloading, editing and publishing all work as in the desktop app. Audio plays in your browser, streamed from the server. Formats the browser can't play, such as ALAC, are converted on the server.

With Docker Compose (see [`docker-compose.yml`](docker-compose.yml)):

```shell
cp .env.example .env   # set LRCGET_MUSIC_DIR, LRCGET_PASSWORD and the UID/GID that own your music
docker compose up -d --build
```

Then open `http://<server>:3000` and add folders under `/music`. The database is kept in the `lrcget-data` Docker volume. If you'd rather bind-mount a host folder to `/data`, create it first and make sure it is owned by `LRCGET_UID`; a folder that Docker creates for you is owned by root, and the server can't write to it.

The server is configured with environment variables:

| Variable | Default | Purpose |
|----------|---------|---------|
| `LRCGET_BIND` | `127.0.0.1:3000` (`0.0.0.0:3000` in Docker) | Listen address |
| `LRCGET_DATA_DIR` | `~/.local/share/lrcget-server` (`/data` in Docker) | Database and transcoding cache |
| `LRCGET_ROOT` | `/` (`/music` in Docker) | Only files and folders below this path can be browsed, scanned or played |
| `LRCGET_MUSIC_DIR` | `LRCGET_ROOT`, or `~/Music` | Folder suggested when setting up a library |
| `LRCGET_USERNAME`, `LRCGET_PASSWORD` | unset | Require HTTP basic auth |
| `LRCGET_WEB_DIR` | `dist/` next to the binary | Built frontend |

The web server has no user accounts. Anyone who can reach it can browse `LRCGET_ROOT`, change lyrics files, and publish to LRCLIB. Set `LRCGET_USERNAME`/`LRCGET_PASSWORD`, or put it behind a reverse proxy that handles authentication, before exposing it beyond localhost. Use HTTPS if you want *Paste LRC* to read your clipboard; browsers only allow that on secure origins.

Without Docker:

```shell
npm install
npm run build
cd src-tauri
cargo build --release --bin lrcget-server --no-default-features --features server
LRCGET_WEB_DIR=../dist ./target/release/lrcget-server
```

## Screenshots

![01.png](https://raw.githubusercontent.com/tranxuanthang/lrcget/9e0578bd9411fcc024a56d2f1108701751c5ec3a/screenshots/01.png)

<details>
<summary>Click to expand more screenshots</summary>

![02.png](https://raw.githubusercontent.com/tranxuanthang/lrcget/9e0578bd9411fcc024a56d2f1108701751c5ec3a/screenshots/02.png)

![03.png](https://raw.githubusercontent.com/tranxuanthang/lrcget/9e0578bd9411fcc024a56d2f1108701751c5ec3a/screenshots/03.png)

![04.png](https://raw.githubusercontent.com/tranxuanthang/lrcget/9e0578bd9411fcc024a56d2f1108701751c5ec3a/screenshots/04.png)

</details>

## Donation

Toss a coin to your developer?

**GitHub Sponsors (Recommended - 100% of your support goes to the developer):**

https://github.com/sponsors/tranxuanthang

**Buy Me a Coffee:**

https://www.buymeacoffee.com/thangtran

**Paypal:**

https://paypal.me/tranxuanthang98

**Monero (XMR):**

```
43ZN5qDdGQhPGthFnngD8rjCHYLsEFBcyJjDC1GPZzVxWSfT8R48QCLNGyy6Z9LvatF5j8kSgv23DgJpixJg8bnmMnKm3b7
```

**Litecoin (LTC):**

```
ltc1q7texq5qsp59gclqlwf6asrqmhm98gruvz94a48
```

## Troubleshooting

**Audio cannot be played in Linux (Ubuntu and other distros)**

Try to install `pipewire-alsa` package. For example, in Ubuntu or Debian-based distros:

```
sudo apt install pipewire-alsa
```

**App won't open in Windows 10/11**

If you are using Windows 10 LTSC, or have tried running some scripts to debloat Windows 10 (which will uninstall Microsoft Edge and its webview component), you might have issues as LRCGET depends on WebView2. Reinstalling Microsoft Edge might fix the problem (see issue https://github.com/tranxuanthang/lrcget/issues/45).

**Scrollbar is invisible in Linux (KDE Plasma 5/6)**

The exact cause is still unknown, but it can be fixed by going to System Settings > Appearance > Global Theme > Application Style > Configure GNOME/GTK Application Style... > Change to something other than breeze (Awaita or Default) > Apply (see comment https://github.com/tranxuanthang/lrcget/issues/44#issuecomment-1962998268)

## Contact

If you prefer to contact by email:

[hoangtudevops@protonmail.com](mailto:hoangtudevops@protonmail.com)

## Development

LRCGET is made with [Tauri](https://tauri.app).

To start developing the application, you need to do the [prerequisites](https://tauri.app/v1/guides/getting-started/prerequisites) steps according to your operating system.

For example, you need the following components to start the development in Windows:

- Microsoft Visual Studio C++ Build Tools
- Rust 1.81.0 or higher
- NodeJS v16.18.0 or higher

Start the development window with the following command:

```shell
cd lrcget
npm install
npm run tauri dev
```

To work on the web version, run the server and the Vite dev server (which forwards `/api` to it) side by side, then open http://localhost:1420:

```shell
cd src-tauri && cargo run --bin lrcget-server --no-default-features --features server
npm run dev
```

## Building

Start the build process with the following command:

```shell
cd lrcget
npm install
npm run tauri build
```

Your built binaries are located at:

```
./src-tauri/target/release/
```

For more detailed instruction, follow the [building guide](https://tauri.app/v1/guides/building/) to build the application according to your OS platform.
