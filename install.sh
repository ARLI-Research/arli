#!/bin/bash
set -e

BOLD="\033[1m"
GREEN="\033[0;32m"
YELLOW="\033[0;33m"
CYAN="\033[0;36m"
NC="\033[0m"

REPO="ARLI-Research/arli"
INSTALL_DIR="${ARLI_INSTALL_DIR:-$HOME/.local/bin}"

echo -e "${BOLD}ARLI — Rust-native AI Agent Harness${NC}"
echo ""

# Detect platform
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="darwin" ;;
    *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *)            echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

mkdir -p "$INSTALL_DIR"

# Build from source (ensures ENSO feature is included)
echo -e "${YELLOW}Building ARLI from source (includes ENSO settlement)...${NC}"
echo ""

if ! command -v cargo &>/dev/null; then
    echo "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

TMPDIR=$(mktemp -d)
echo "Cloning ARLI..."
git clone --depth 1 "https://github.com/$REPO.git" "$TMPDIR" 2>/dev/null
cd "$TMPDIR"
echo "Compiling with ENSO support (this may take a few minutes)..."
cargo build -p arli-cli --features arli-core/enso --release
cp target/release/arli "$INSTALL_DIR/"
rm -rf "$TMPDIR"
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
