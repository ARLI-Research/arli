#!/bin/bash
set -e

BOLD="\033[1m"
GREEN="\033[0;32m"
YELLOW="\033[0;33m"
CYAN="\033[0;36m"
RED="\033[0;31m"
NC="\033[0m"

REPO="ARLI-Research/arli"
INSTALL_DIR="${ARLI_INSTALL_DIR:-$HOME/.local/bin}"

echo -e "${BOLD}ARLI — Rust-native AI Agent Harness${NC}"
echo ""

# Detect platform
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
    Linux)  ;;
    Darwin) ;;
    *)      echo -e "${RED}Unsupported OS: $OS${NC}"; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64) ;;
    aarch64|arm64) ;;
    *)            echo -e "${RED}Unsupported arch: $ARCH${NC}"; exit 1 ;;
esac

mkdir -p "$INSTALL_DIR"

# Check prerequisites
if ! command -v git &>/dev/null; then
    echo -e "${RED}git not found. Install git first: https://git-scm.com/${NC}"
    exit 1
fi

if ! command -v cargo &>/dev/null; then
    echo -e "${YELLOW}Installing Rust toolchain...${NC}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

echo -e "${YELLOW}Building ARLI from source (includes ENSO settlement)...${NC}"
echo ""

TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

echo "Cloning ARLI from GitHub..."
git clone --depth 1 --progress "https://github.com/${REPO}.git" "$TMPDIR" 2>&1 | tail -1

cd "$TMPDIR"

# arli-trading requires hypersdk (local path dep not in this repo).
# Temporarily exclude it from the workspace for a standalone build.
if [[ "$OS" = "Darwin" ]]; then
    sed -i '' 's/    "arli-trading",/    #"arli-trading",/' Cargo.toml
else
    sed -i 's/    "arli-trading",/    #"arli-trading",/' Cargo.toml
fi

echo "Compiling with ENSO support (3-8 minutes)..."
cargo build -p arli-cli --features arli-core/enso --release

cp target/release/arli "$INSTALL_DIR/"
echo -e "${GREEN}Installed to $INSTALL_DIR/arli${NC}"

# Add to PATH if needed
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.bashrc"
    echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.zshrc" 2>/dev/null || true
    echo -e "${YELLOW}Added $INSTALL_DIR to PATH${NC}"
    echo -e "Run: ${CYAN}source ~/.zshrc${NC}  (or restart terminal)"
fi

echo ""
echo -e "${BOLD}═══ Next steps ═══${NC}"
echo ""
echo -e "  ${CYAN}arli enso onboard${NC}    — ENSO settlement setup (key + register)"
echo -e "  ${CYAN}arli enso run -c <id>${NC} — attest + settle a contract"
echo ""
echo -e "${BOLD}General usage:${NC}"
echo -e "  ${CYAN}arli chat${NC}            — interactive AI assistant"
echo ""
echo -e "${GREEN}Done.${NC}"
