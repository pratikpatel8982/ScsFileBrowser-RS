# ScsFileBrowser (Rust)

A cross-platform Rust rewrite of [Callum-Bell/ScsFileBrowser](https://github.com/Callum-Bell/ScsFileBrowser):
a browser for the `.scs` (HashFS v1 and v2), `.zip` and `.mp` archives used by
Euro Truck Simulator 2 and American Truck Simulator.

## Features
- Opens a single archive or a whole folder of them, merged into one virtual
  filesystem (later archives in case-insensitive name order override earlier ones;
  the Source column shows `name (+N)` when several archives contain a file).
- Folder and file tables with sortable columns, multi-select (click, Ctrl, Shift).
- Breadcrumb path bar; click it to type a path. Backspace goes up one level.
- Extract files or whole folders (background thread, cancellable), with
  automatic decryption of 3nK-encrypted `.sii` files.
- "Open" / "Open With" external programs, configured in Settings
  (programs can be grouped by file extension, drag and drop to organise).
- Hold Shift while right-clicking for "Copy CityHash Value" and
  "Copy sub folder/file names".
- Drag and drop an archive (`.scs`, `.zip`, `.mp`) onto the window to open it.
- Drag files out of the file list into Explorer / Finder / another app (they are
  extracted to a temp folder first; encrypted SII files are decrypted). On Linux this
  uses the X11 XDND protocol (works with the common file managers); on Windows and
  macOS it uses the OS drag APIs.
- Rosé Pine dark theme, recent-paths menu, and a log file (`log.log`).

## Build
Requires a recent stable Rust toolchain (https://rustup.rs).

```sh
cargo build --release
./target/release/scs-file-browser [path-to-archive-or-folder]
```

Platform notes:
- **Windows**: nothing extra. The release binary has no console window.
- **macOS**: nothing extra.
- **Linux**: install the usual windowing headers, e.g. on Debian/Ubuntu
  `sudo apt install libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgl1-mesa-dev`.
  Drag and drop needs X11: on a Wayland session the app automatically runs through
  XWayland (set `SCS_FORCE_WAYLAND=1` to keep native Wayland, without drag and drop).
  File dialogs use the xdg-desktop-portal (via `rfd`), so a portal backend
  (GTK, KDE, ...) must be running.

Headless library only (no GUI dependencies):

```sh
cargo build --no-default-features
```

## Test
```sh
cargo test                        # everything
cargo test --no-default-features  # core library only
```
The suite builds synthetic HashFS v1/v2 and ZIP archives, and checks the CityHash
implementation against 609 reference vectors from the C implementation the
original C# code was derived from.

## Settings and logs
Stored in the per-user local data directory under `TruckSim/ScsFileBrowser/`: `settings.json`
(same field names as the original) and `log.log`.

## Known limitations
- Verified against synthetic archives, not real game archives. HashFS v2
  metadata handling follows the format as documented/understood; please report any
  real `.scs` that fails to open.
- ZIP64 and encrypted ZIP entries are unsupported (same as the original).
- Dragging files out on Linux needs X11 (the app uses XWayland on Wayland sessions); there is no drag cursor/preview image on Linux.
- Large drags extract on the UI thread, so the window pauses until the files are ready.
- Zstd-compressed entries are detected and reported but not decompressed.
- GUI testing was done under Xvfb with software rendering on Linux only;
  Windows and macOS builds are untested here (CI workflow included).
