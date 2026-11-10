# Bee Launcher

A Windows-first Tauri launcher with a full-bleed Minecraft home screen, custom Bee Launcher branding, and a frameless resizable window. Its Settings area is organized into Profile, Game, Appearance, Minecraft folder, and About pages. It scans an existing `.minecraft` directory and launches real installed Java profiles, including Fabric; it does not show demo profiles or simulate a game launch. The wallpaper and profile picture can be customized and kept locally on that PC.

## What it does

- Scores `%LOCALAPPDATA%\.minecraft`, `%APPDATA%\.minecraft`, and `%USERPROFILE%\.minecraft` by their actual contents. TLauncher markers such as `TlauncherProfiles.json`, `TLauncher.exe`, `.fabric`, populated `versions`, `libraries`, and `runtime` folders let the populated Roaming install win over an empty LocalAppData placeholder. Old automatic picks are re-evaluated; a folder chosen manually is kept.
- Lets you choose a different `.minecraft` folder and remembers it in the app's own settings directory.
- Imports standard Modrinth `.mrpack` packs and Bee Launcher bundle `.zip` files from **Mods → Import a modpack** (also available in Installations). It resolves the pinned Minecraft version and Fabric Loader profile, installs the client mods, overrides, Minecraft libraries, client JAR, and asset files, and checks the pack's SHA-1/SHA-512 hashes where supplied. The import panel shows download progress and any specific failure.
- Imported packs are stored as selectable profiles under Bee Launcher's own app-data `instances` folder, not merged into the chosen `.minecraft` folder. Existing profile files, root `mods`, `config`, and `saves` are not moved or overwritten. Fabric and vanilla Modrinth packs are supported; Forge, NeoForge, and Quilt imports are not yet supported.
- Lists readable local profiles (including spaced names such as `versions/Fabric 26.3/`), reads each profile's own JSON and local JAR, follows only an explicit `inheritsFrom`, parses Mojang/TLauncher `values` and `default_user_jvm` arguments, and builds a local library classpath.
- Uses the selected `.minecraft` root as the game directory so existing root `mods`, `config`, `saves`, and assets are available to Fabric. Before launch, it checks the metadata-declared client JAR and platform-specific libraries under `.minecraft/libraries`, downloads missing or damaged files over HTTPS, and verifies declared sizes and SHA-1 hashes. It tries Maven Central mirrors only for non-Mojang Maven artifacts, and still requires the profile's exact checksum. It extracts only the current platform's native DLLs/shared libraries into the profile's `natives` folder. Worlds, mods, configs, and saves are left untouched. Java output is written to `.minecraft/logs/blockline-latest.log`; if Java exits during startup, the launcher shows the exit code and recent log lines instead of claiming success.
- Explicitly verifies the profile-declared `jopt-simple` JAR contains `joptsimple/ValueConverter` and ensures it is on the final Java classpath. The startup log records the Java executable, resolved classpath, and launch arguments for future diagnosis.
- Embeds the exact `com.mojang:authlib:10.0.77` JAR declared by Fabric 26.3 (152,645 bytes; SHA-1 `add8754cda96cf0cd840441632875648836f8a71`) and installs it to `.minecraft/libraries/com/mojang/authlib/10.0.77/` before resolving libraries, so it needs no Authlib network download.
- Uses a local player name and offline UUID. It does not read or store TLauncher or Microsoft account tokens; online servers requiring authenticated accounts are not supported by this local profile.
- Main Play and Quick Play both launch the currently selected installed profile directly; Quick Play does not open a separate launcher or browser.
- On Windows, the launcher prefers `javaw.exe` for the game process to avoid a transient console window when that runtime is available. Java version detection still validates the selected runtime first.
- Windows Java checks and version-range lookups now suppress helper console windows; the OS version is cached so repeated profile-argument checks do not open repeated console processes.
- On Windows, the running process declares the stable `com.blockline.launcher` AppUserModelID before creating its window, then applies the embedded Bee icon to the HWND, Win32 window class, and Tauri window. The installer writes that same AppUserModelID and the Bee ICO to its Start menu and desktop shortcuts, and updates any existing pinned shortcut that targets the installed executable, so pins group with the running app and use the Bee icon too.
- The Windows title bar is removed. The app provides custom minimize, maximize/restore, close, move, and resize controls over the wallpaper.
- The launcher remembers its last normal window size and restores it on the next start. This is stored separately from the Minecraft game resolution in Settings; temporary maximized dimensions are not saved as the normal size.

## Build on Windows

Install Node.js, pnpm, Rust, Microsoft C++ Build Tools (Desktop development with C++), and WebView2. From this directory run:

```powershell
pnpm install
pnpm tauri:dev
```

Create installers with:

```powershell
pnpm tauri:build
```

The MSI and NSIS installers are written under `src-tauri/target/release/bundle/`.

## Import a modpack

Open **Mods → Choose .mrpack or ZIP** (or **Installations → Import Modpack**), then select either the original Modrinth `.mrpack` or a Bee Launcher bundle ZIP. A standard `.mrpack` downloads the indexed files from their HTTPS sources; a Bee ZIP installs its bundled mod files after matching them against the embedded Modrinth manifest. Once complete, choose the imported profile from the Play screen's profile selector. The pack installs to an isolated Bee Launcher instance; it does not alter the existing Minecraft world's saves, mods, or configs. The required Java major version is shown for each profile (Minecraft 26.3 requires Java 25).

## Notes

- Java detection prefers the selected profile's declared runtime component and requires the declared major version. Other installed runtimes are considered only if they match that major version; the uploaded Fabric 26.3 profile requires Java 25 (`java-runtime-epsilon`).
- Jinuk Hand is used for readable interface text; Pixelify Sans is reserved for the Play button and small game-style accents. The supplied bee logo and wallpaper artwork are bundled locally for offline use. Font license notices for Noto Sans and Pixelify Sans are included in `src/assets/fonts/`.
- Profiles remain visible if files need repair. A profile with a valid client download URL can be played even when its local client JAR is missing; a profile without a main class or usable client-JAR metadata is marked not ready.
- Minecraft is a Mojang/Microsoft product. Bee Launcher is an independent launcher UI and is not affiliated with Mojang or Microsoft.
- Jinuk Hand is used for readable interface text; Pixelify Sans is reserved for the Play button and small game-style accents. The supplied bee artwork is used only for the Windows app and installer icons; the Minecraft wordmark and wallpaper artwork are bundled locally for offline use. Font license notices for Noto Sans and Pixelify Sans are included in `src/assets/fonts/`.
