#!/bin/bash
set -e

# Colors
BOLD="\033[1m"
GREEN="\033[0;32m"
YELLOW="\033[0;33m"
CYAN="\033[0;36m"
NC="\033[0m"

REPO="ARLI-Research/arli"
INSTALL_DIR="${ARLI_INSTALL_DIR:-$HOME/.local/bin}"
BINARY="arli"
GITHUB_API="https://api.github.com/repos/$REPO"
GITHUB_DL="https://github.com/$REPO/releases/download"

echo -e "${BOLD}ARLI — Rust-native AI Agent Harness${NC}"
echo ""

# Detect platform
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="macos" ;;
    *)      echo "Unsupported OS: $OS"; PLATFORM="unknown" ;;
esac

case "$ARCH" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *)             echo "Unsupported arch: $ARCH"; ARCH="unknown" ;;
esac

mkdir -p "$INSTALL_DIR"

# Try download pre-built binary
VERSION=$(curl -fsSL "$GITHUB_API/releases/latest" 2>/dev/null | grep '"tag_name"' | cut -d'"' -f4 || echo "")

if [ -n "$VERSION" ] && [ "$PLATFORM" != "unknown" ]; then
    TARBALL="arli-${PLATFORM}-${ARCH}.tar.gz"
    URL="$GITHUB_DL/$VERSION/$TARBALL"
    echo "Downloading ARLI $VERSION for $PLATFORM/$ARCH..."
    
    if curl -fsSL "$URL" -o /tmp/arli.tar.gz 2>/dev/null; then
        tar xzf /tmp/arli.tar.gz -C "$INSTALL_DIR"
        rm /tmp/arli.tar.gz
        chmod +x "$INSTALL_DIR/$BINARY" 2>/dev/null || true
        echo -e "${GREEN}Installed to $INSTALL_DIR/$BINARY${NC}"
    else
        echo -e "${YELLOW}No pre-built binary for $PLATFORM/$ARCH${NC}"
        VERSION=""
    fi
fi

# Fallback: build from source
if [ -z "$VERSION" ]; then
    if ! command -v cargo &>/dev/null; then
        echo "Installing Rust toolchain..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "$HOME/.cargo/env"
    fi
    
    echo "Building ARLI from source..."
    TMPDIR=$(mktemp -d)
    git clone --depth 1 "https://github.com/$REPO.git" "$TMPDIR" 2>/dev/null
    cd "$TMPDIR"
    cargo build --release 2>&1 | tail -3
    cp target/release/$BINARY "$INSTALL_DIR/"
    rm -rf "$TMPDIR"
    echo -e "${GREEN}Installed to $INSTALL_DIR/$BINARY${NC}"
fi

# Add to PATH if needed
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo "export PATH="$INSTALL_DIR:\$PATH"" >> "$HOME/.bashrc"
    echo "export PATH="$INSTALL_DIR:\$PATH"" >> "$HOME/.zshrc" 2>/dev/null || true
    echo -e "${YELLOW}Added $INSTALL_DIR to PATH in .bashrc${NC}"
fi

echo ""
echo -e "${BOLD}Next step: ${CYAN}arli setup${NC}  — configure API keys"
echo -e "${BOLD}Then:    ${CYAN}arli chat${NC}   — start chatting"
echo ""
echo -e "${GREEN}ARLI installed.${NC}"
