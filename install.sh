#!/usr/bin/env bash
# decryptor install.sh — set up everything the linux tui needs, natively.
#
#   ./install.sh          build & install ctrdecrypt, decryptor-tui, makerom, ctrtool
#   --no-tools            skip the native makerom/ctrtool build (already installed)
#   --tools-only          only build makerom/ctrtool
#   --no-seed             skip the seeddb.bin download prompt
#   --help                show this help
set -euo pipefail

PREFIX="${HOME}/.local"
BIN_DIR="${PREFIX}/bin"
BUILD_DIR="${HOME}/.cache/decryptor-build"
TOOLS_ONLY=0
NO_TOOLS=0
NO_SEED=0

for arg in "$@"; do
	case "$arg" in
	--no-tools)   NO_TOOLS=1 ;;
	--tools-only) TOOLS_ONLY=1 ;;
	--no-seed)    NO_SEED=1 ;;
	--help|-h)
		sed -n '2,9p' "$0"
		exit 0 ;;
	*) echo "unknown option: $arg (see --help)"; exit 1 ;;
	esac
done

say()  { printf '\n\033[1;36m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m   ✓\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m   !\033[0m %s\n' "$*"; }

# ---------------------------------------------------------------- 1. distro pkgs
PKG_MANAGER=""
for candidate in apt-get dnf pacman zypper; do
	if command -v "$candidate" >/dev/null 2>&1; then
		PKG_MANAGER="$candidate"
		break
	fi
done

MISSING=()
for dep in make gcc pkg-config curl; do
	command -v "$dep" >/dev/null 2>&1 || MISSING+=("$dep")
done

if [ "${#MISSING[@]}" -gt 0 ]; then
	if [ -n "$PKG_MANAGER" ]; then
		say "installing build dependencies (${MISSING[*]}) — sudo may ask for your password"
		case "$PKG_MANAGER" in
		apt-get) sudo apt-get update && sudo apt-get install -y build-essential pkg-config curl ;;
		dnf)     sudo dnf install -y make gcc gcc-c++ pkgconf-pkg-config curl ;;
		pacman)  sudo pacman -S --needed base-devel pkgconf curl ;;
		zypper)  sudo zypper install -y make gcc gcc-c++ pkg-config curl ;;
		esac
	else
		warn "missing: ${MISSING[*]} and no known package manager found — install them manually"
	fi
else
	ok "build dependencies present"
fi

# ---------------------------------------------------------------- 2. rust
if ! command -v cargo >/dev/null 2>&1; then
	say "rustup/cargo missing — installing rust (enter at the prompt)"
	curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
	export PATH="${HOME}/.cargo/bin:${PATH}"
fi
ok "cargo $(cargo --version | cut -d' ' -f2)"

mkdir -p "${BIN_DIR}"
HERE="$(cd "$(dirname "$0")" && pwd)"

# ---------------------------------------------------------------- 3. project_ctr
if [ "${NO_TOOLS}" -eq 0 ]; then
	say "building makerom & ctrtool (native linux, replaces the .exe's)"
	mkdir -p "${BUILD_DIR}"
	for tool in ctrtool makerom; do
		if [ -x "${BIN_DIR}/${tool}" ]; then
			ok "${tool} already installed"
			continue
		fi
		if [ ! -d "${BUILD_DIR}/Project_CTR" ]; then
			git clone --depth 1 https://github.com/3DSGuy/Project_CTR "${BUILD_DIR}/Project_CTR"
		fi
		(cd "${BUILD_DIR}/Project_CTR/${tool}" && make -j"$(nproc)" deps >/dev/null && make -j"$(nproc)")
		install -Dm755 "${BUILD_DIR}/Project_CTR/${tool}/bin/${tool}" "${BIN_DIR}/${tool}"
		ok "installed ${BIN_DIR}/${tool}"
	done
fi

# ---------------------------------------------------------------- 4. rust tools
if [ "${TOOLS_ONLY}" -eq 0 ]; then
	say "building ctrdecrypt (library + cli)"
	(cd "${HERE}/ctrdecrypt" && cargo build --release)
	install -Dm755 "${HERE}/ctrdecrypt/target/release/ctrdecrypt" "${BIN_DIR}/ctrdecrypt"
	ok "installed ${BIN_DIR}/ctrdecrypt"

	say "building decryptor-tui (tui + headless)"
	(cd "${HERE}/tui" && cargo build --release)
	install -Dm755 "${HERE}/tui/target/release/decryptor-tui" "${BIN_DIR}/decryptor-tui"
	ok "installed ${BIN_DIR}/decryptor-tui"
fi

# ---------------------------------------------------------------- 5. seeddb
if [ "${NO_SEED}" -eq 0 ] && [ "${TOOLS_ONLY}" -eq 0 ]; then
	for cand in "${BIN_DIR}/seeddb.bin" "${HOME}/3ds/seeddb.bin" "${HOME}/.3ds/seeddb.bin"; do
		if [ -f "$cand" ]; then
			ok "seeddb.bin found at ${cand}"
			GOT_SEED=1
			break
		fi
	done
	if [ "${GOT_SEED:-0}" -eq 0 ]; then
		printf 'download seeddb.bin now? [y/N] '
		read -r answer
		if [ "$answer" = "y" ] || [ "$answer" = "Y" ]; then
			mkdir -p "${HOME}/3ds"
			curl -fL --progress-bar \
				-o "${HOME}/3ds/seeddb.bin" \
				https://raw.githubusercontent.com/ihaveamac/3DS-rom-tools/master/seeddb.bin
			ok "saved ${HOME}/3ds/seeddb.bin"
		else
			warn "skipped — get it later inside the app with [d], or ~/3ds/seeddb.bin"
		fi
	fi
fi

# ---------------------------------------------------------------- done
say "installed to ${BIN_DIR}"
[ -d "${HOME}/.local/bin" ] || true
case ":${PATH}:" in
	*":${BIN_DIR}:"*) : ;;
	*) warn "${BIN_DIR} is not on your PATH — add 'export PATH=\"${HOME}/.local/bin:\$PATH\"' to your shell rc" ;;
esac

cat <<EOF

run:   decryptor-tui /path/to/roms     # the interface
       decryptor-tui --headless DIR   # for scripts
docs:  see docs/USAGE.md
EOF
