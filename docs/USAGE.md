# decryptor-tui — step by step

A short, practical guide from nothing to decrypted files.

---

## 1. Install (Linux)

Open a terminal in the decryptor directory and run:

```console
$ ./install.sh
```

What it does:

1. checks for `cargo`, `make`, `gcc`, `pkg-config` (helps you install them on
   apt/dnf/pacman/zypper distros),
2. builds `ctrtool` and `makerom` from Project_CTR (the native Linux ports of
   the Windows-only tools the old `.bat` file used),
3. builds `ctrdecrypt` and `decryptor-tui` with cargo,
4. installs everything into `~/.local/bin` (make sure that folder is on your
   `PATH`; openSUSE/Fedora/Arch usually include it),
5. offers to download `seeddb.bin` (needed for seed-encrypted titles from
   firmware ≥ 9.6).

## 2. Where to put seeddb.bin (if you skipped step 5)

Any of these locations is picked up automatically:

* the directory you run in,
* `~/3ds/seeddb.bin` ← what the app downloads to,
* `~/.3ds/seeddb.bin`.

Or hit `d` inside the TUI at any time.

## 3. Run it

```console
$ decryptor-tui ~/3ds/roms          # or any folder with .cia/.3ds/.cci/.3dz
```

You'll see the file inventory: name, title id, kind (game / update / DLC /
DSiWare / system), version, size, and whether it still needs decrypting.

Already-decrypted files stay untouched (they are detected, not guessed).

## 4. Decrypt

* `enter` — decrypt the file under the cursor,
* `a` — decrypt **all** pending files in the folder,
* sit back: progress bars show make-model progress per file; the log pane
  shows what's happening.

Outputs land next to the originals:

| input                | output                            |
|----------------------|-----------------------------------|
| `Game.3ds` / `Game.cci` | `Game-decrypted.cci`           |
| `Game.cia`           | `Game-decrypted.cia`              |
| `Game-v1.1 (Update).cia`? update tids | `Game (Update)-decrypted.cia` |
| DLC `.cia`           | `Game (DLC)-decrypted.cia`        |

Intermediates (`.ncch` part files) are deleted after repacking. Keep them with
`--keep-intermediates` or `settings → keep intermediates`.

## 5. The menu (`m`)

* run all pending files
* rescan directory
* settings — change rom directory, set exact `seeddb.bin`/`makerom` paths,
  toggle online seed lookup, toggle intermediate cleanup
* seeddb downloads — fetch the seed database right here (`enter`)
* help — every key on one screen
* about — version & credits

## 6. Scripting it

Every GUI step has a flag:

```console
$ decryptor-tui --headless /sd/3ds           # decrypt the whole folder
$ decryptor-tui --headless --no-online ~/3ds # never touch the internet
$ decryptor-tui --headless \
    --seeddb ~/3ds/seeddb.bin \
    --makerom ~/.local/bin/makerom \
    /mnt/backup/cartridges
```

Exit code is `0` when everything worked — easy to script.

## 7. Troubleshooting

| symptom                                       | fix                                                |
|-----------------------------------------------|----------------------------------------------------|
| `makerom ✗ missing` in the footer             | run `./install.sh` again (or `settings` → makerom path) |
| `seeddb ✗ not found`                          | press `d` → download, or set path, or pass `--seeddb` |
| "unknown crypto state"                        | file is odd (WD homebrew/intro CIA?) — try `--keep-intermediates` and report it |
| ssh session cut mid-decrypt                   | use `tmux`/`screen`; interrupted dumps leave partial `.ncch` files — delete them and re-run |

Nothing here writes outside the rom directory (plus `~/3ds/seeddb.bin` on
downloads & `$HOME/.local/bin` during install).
