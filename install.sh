#!/bin/bash
set -eo pipefail

BOLD="\033[1m"
GREEN="\033[0;32m"
YELLOW="\033[0;33m"
CYAN="\033[0;36m"
RED="\033[0;31m"
NC="\033[0m"

REPO="ARLI-Research/arli"
VERSION="${ARLI_VERSION:-v0.5.4}"
INSTALL_DIR="${ARLI_INSTALL_DIR:-$HOME/.local/bin}"

echo -e "${BOLD}ARLI — Production-Grade AI Agent Harness${NC}"
echo ""

# Detect platform
OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
    Linux)  OS_LOWER="linux" ;;
    Darwin) OS_LOWER="macos" ;;
    *)      echo -e "${RED}Unsupported OS: $OS${NC}"; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64)  ARCH_LOWER="amd64" ;;
    aarch64|arm64) ARCH_LOWER="arm64" ;;
    *)             echo -e "${RED}Unsupported arch: $ARCH${NC}"; exit 1 ;;
esac

mkdir -p "$INSTALL_DIR"

# Try pre-built binary first
BINARY_NAME="arli-${OS_LOWER}-${ARCH_LOWER}.tar.gz"
RELEASE_URL="https://github.com/${REPO}/releases/download/${VERSION}/${BINARY_NAME}"

echo -e "${YELLOW}Downloading ARLI ${VERSION}...${NC}"
if command -v curl &>/dev/null; then
    HTTP_CODE=$(curl -sL -o /tmp/arli.tar.gz -w "%{http_code}" "$RELEASE_URL")
elif command -v wget &>/dev/null; then
    HTTP_CODE=$(wget -q -O /tmp/arli.tar.gz --server-response "$RELEASE_URL" 2>&1 | awk '/HTTP\// {print $2}' | tail -1)
else
    HTTP_CODE="404"
fi

if [ "$HTTP_CODE" = "200" ]; then
    tar -xzf /tmp/arli.tar.gz -C "$INSTALL_DIR"
    rm -f /tmp/arli.tar.gz
    chmod +x "$INSTALL_DIR/arli"
    echo -e "${GREEN}Installed to $INSTALL_DIR/arli${NC}"
else
    # Fall back to source build
    rm -f /tmp/arli.tar.gz 2>/dev/null || true
    echo -e "${YELLOW}No pre-built binary for ${OS_LOWER}-${ARCH_LOWER}. Building from source.${NC}"
    echo ""

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

    TMPDIR=$(mktemp -d)
    trap "rm -rf $TMPDIR" EXIT

    echo "Cloning ARLI from GitHub..."

    clone_arli() {
        git clone --depth 1 --progress "https://github.com/${REPO}.git" "$TMPDIR" 2>&1 | tail -1
    }

    for attempt in 1 2 3; do
        if clone_arli && [ -f "$TMPDIR/Cargo.toml" ]; then
            break
        fi
        echo -e "${YELLOW}Clone attempt $attempt failed, retrying...${NC}"
        rm -rf "$TMPDIR"
        mkdir "$TMPDIR"
        sleep 2
    done

    if [ ! -f "$TMPDIR/Cargo.toml" ]; then
        echo -e "${RED}Failed to clone ARLI after 3 attempts.${NC}"
        echo -e "${YELLOW}Check your network connection or try:${NC}"
        echo -e "  git clone https://github.com/${REPO}.git"
        exit 1
    fi

    cd "$TMPDIR"

    # arli-trading depends on hypersdk (../../hypersdk) which isn't in this repo.
    # Create a minimal stub so Cargo can parse the workspace.
    mkdir -p "$TMPDIR/../hypersdk/src"
    cat > "$TMPDIR/../hypersdk/Cargo.toml" << 'HYPEREOF'
[package]
name = "hypersdk"
version = "0.1.0"
edition = "2021"
HYPEREOF
    echo "" > "$TMPDIR/../hypersdk/src/lib.rs"

    echo "Compiling with ENSO support (3-8 minutes)..."
    cargo build -p arli-cli --features arli-core/enso --release

    cp target/release/arli "$INSTALL_DIR/"
    echo -e "${GREEN}Installed to $INSTALL_DIR/arli${NC}"
fi

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
echo -e "  ${CYAN}arli setup${NC}          — configure providers + platforms"
echo -e "  ${CYAN}arli chat${NC}           — interactive AI assistant"
echo -e "  ${CYAN}arli harness analyze${NC} — telemetry + recommendations"
echo -e "  ${CYAN}arli enso setup${NC}     — ENSO settlement (key + register)"
echo ""
echo -e "${GREEN}Done.${NC}"
