#!/bin/bash
set -e

BOLD="\033[1m"
GREEN="\033[0;32m"
YELLOW="\033[0;33m"
CYAN="\033[0;36m"
NC="\033[0m"

REPO="ARLI-Research/arli"
INSTALL_DIR="${ARLI_INSTALL_DIR:-$HOME/.local/bin}"
GITHUB_DL="https://github.com/$REPO/releases/latest/download"

echo -e "${BOLD}ARLI — Rust-native AI Agent Harness${NC}"
echo ""

# Detect platform
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="linux" ;;
    *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *)            echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

mkdir -p "$INSTALL_DIR"

# Try downloading pre-built binary
DOWNLOADED=false
if [ "$OS" = "Linux" ]; then
    TARBALL="arli-linux-x86_64.tar.gz"
    URL="$GITHUB_DL/$TARBALL"
    echo "Downloading ARLI..."

    if curl -fsSL --progress-bar "$URL" -o /tmp/arli.tar.gz 2>/dev/null; then
        tar xzf /tmp/arli.tar.gz -C "$INSTALL_DIR"
        rm /tmp/arli.tar.gz
        chmod +x "$INSTALL_DIR/arli" "$INSTALL_DIR/arli-gateway" 2>/dev/null || true
        echo -e "${GREEN}Installed to $INSTALL_DIR/arli${NC}"
        DOWNLOADED=true
    fi
fi

# Fallback: build from source
if [ "$DOWNLOADED" = false ]; then
    echo -e "${YELLOW}Building from source (requires Rust)...${NC}"

    if ! command -v cargo &>/dev/null; then
        echo "Installing Rust toolchain..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "$HOME/.cargo/env"
    fi

    TMPDIR=$(mktemp -d)
    echo "Cloning ARLI..."
    git clone --depth 1 "https://github.com/$REPO.git" "$TMPDIR" 2>/dev/null
    cd "$TMPDIR"
    echo "Compiling (this may take a few minutes)..."
    cargo build --release
    cp target/release/arli "$INSTALL_DIR/"
    cp target/release/arli-gateway "$INSTALL_DIR/"
    rm -rf "$TMPDIR"
    echo -e "${GREEN}Built and installed to $INSTALL_DIR/arli${NC}"
fi

# Add to PATH if needed
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.bashrc"
    echo "export PATH=\"$INSTALL_DIR:\$PATH\"" >> "$HOME/.zshrc" 2>/dev/null || true
    echo -e "${YELLOW}Added $INSTALL_DIR to PATH${NC}"
    echo -e "Run: ${CYAN}source ~/.zshrc${NC}  (or restart terminal)"
fi

echo ""
echo -e "${BOLD}Next:  ${CYAN}arli setup${NC}     — configure API keys + Telegram"
echo -e "${BOLD}Then:  ${CYAN}arli chat${NC}      — start chatting"
echo -e "${BOLD}Or:    ${CYAN}arli-gateway${NC}  — Telegram bot"
echo ""
echo -e "${GREEN}Done.${NC}"
