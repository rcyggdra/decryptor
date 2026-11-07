# Batch CIA CCI Decryptor

Batch decrypting for Nintendo 3DS games and applications (`.3ds`|`.cci`, `.cia`).

Azahar makes use of the `.cci` extension, which is the true name of the format used by `.3ds` files.

Batch CIA CCI Decryptor is a rewritten version of the Batch CIA 3DS Decryptor by matiffeder.

Original thread: https://gbatemp.net/threads/batch-cia-3ds-decryptor-a-simple-batch-file-to-decrypt-cia-3ds.512385/

## Features

* DLC/Patch CIA → Decrypted CIA, able to install in Azahar
* 3DS Games → Decrypted and trimmed 3DS, so it is smaller
* CIA Games → Decrypted CCI (NCSD), not CXI (NCCH)
* Auto detect CIA type (DLC/Patch/Game)

## This fork: a Linux TUI (branch `tui`)

Everything the Windows `.bat` pipeline does — probing, decrypting, repacking with
makerom, seeddb lookup — now runs natively on **any Linux distro**, driven by
one interactive TUI or a script-friendly headless mode:

* `decryptor-tui` — a plain-terminal interface (no window server needed):
  a file list with live progress bars + per-file status and a scrolling log.
  * "everything with options" menus: settings (directory, seeddb.bin, makerom
    paths, online-seed toggle, keep-intermediates), seeddb downloads right in
    the app, help & about modals.
  * in-process probing: each rom shows its title id, kind (game / update /
    DLC / DSiWare / system) and whether it is still encrypted — no external
    tools needed for detection.
  * files already decrypted are detected and skipped automatically.
  * updates get repacked as `Name (Update)-decrypted.cia`, DLC as
    `Name (DLC)-decrypted.cia` — same naming as the `.bat` you know.
  * animated spinners & progress bars, runs fine over SSH.
* `decryptor-tui --headless DIR` — the same pipeline, plain output + exit codes,
  for scripts and automation.
* `ctrdecrypt` — the decryptor core, now also a **library** (`ctrdecrypt` crate
  with `decrypt()` + `probe()`), so other tools (like
  [ihaveacreditcard](https://github.com/visadaking/ihaveacreditcard)) can drive
  it in-process.
* Native `makerom` + `ctrtool` builds (Project_CTR) are fetched and installed by
  `install.sh`, so no Windows binaries or WINE are needed.

## Quick start (Linux)

```console
$ ./install.sh                 # builds & installs everything to ~/.local/bin
$ decryptor-tui /path/to/roms  # pick the TUI
$ decryptor-tui --headless /path/to/roms   # or automate it
```

`--help` lists all options; `docs/USAGE.md` is the step-by-step guide.

## Windows

The original Windows workflow is untouched: grab a release, unpack, drop your
CIA/3DS files next to the batch file, then run the `.bat`. (See `bin/`.)

## Building from source (Linux)

Needs `cargo` (rustup), `make`, `gcc` and `pkg-config`.

```console
$ make -C ctrdecrypt-release   # not needed — just use:
$ cargo build --release --manifest-path ctrdecrypt/Cargo.toml
$ cargo build --release --manifest-path tui/Cargo.toml
```

Project_CTR (`ctrtool`, `makerom`) build on their own; `install.sh` automates
that too.

## Credits

* ctrdecrypt by rcyggdra (upstream) — all real decryption work
* ihaveamed/ihaveacreditcard — UX inspiration, seeddb.bin hosting
* 3DSGuy / Project_CTR — makerom & ctrtool
* matiffeder — the original batch decryptor
* Maidware-Project — visual inspiration for the TUI palette
