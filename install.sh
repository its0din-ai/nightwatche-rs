#!/bin/bash
set -e

# Ensure SCRIPT_DIR is correctly set regardless of how the script is called (e.g., with sudo)
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"

# --- Configuration ---
LOGWATCHER_USER="logwatcher"
SERVICE_FILE="/etc/systemd/system/nightwatcher.service"
NIGHTWATCHER_BINARY_NAME="nightwatche-rs"
DEFAULT_INSTALL_DIR="/opt/nightwatcher" # New default installation directory

# Mock URLs for pre-built binaries and checksum file
declare -A MOCK_RELEASE_URLS
MOCK_RELEASE_URLS=(
    ["x86_64"]="https://example.com/nightwatche-rs/nightwatche-rs-linux-amd64"
    ["aarch64"]="https://example.com/nightwatche-rs/nightwatche-rs-linux-arm64"
)
SHA256SUMS_URL="https://example.com/nightwatche-rs/prebuilt.sha256" # Mock URL for the combined checksum file

# --- Argument Parsing ---
BUILD_REQUESTED=false
INSTALL_DIR="$DEFAULT_INSTALL_DIR" # Initialize with default

# Parse command line arguments
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --build)
            BUILD_REQUESTED=true
            echo "--- User explicitly requested build from source. ---"
            ;;
        --install-dir)
            if [ -n "$2" ] && [[ "$2" != --* ]]; then
                INSTALL_DIR="$(readlink -f "$2")" # Resolve to absolute path
                echo "--- Custom installation directory specified: $INSTALL_DIR ---"
                shift # Consume argument value
            else
                echo "Error: --install-dir requires a directory path."
                exit 1
            fi
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
    shift # Consume argument name
done

if ! "$BUILD_REQUESTED"; then
    echo "--- Checking for available pre-built binary. ---"
fi

echo "--- Starting Nightwatcher Service Setup ---"

# 1. Check and Create logwatcher user
echo "1. Checking for user '$LOGWATCHER_USER'..."
if ! id -u "$LOGWATCHER_USER" &>/dev/null; then
    echo "   User '$LOGWATCHER_USER' not found. Creating system user..."
    sudo useradd --system --no-create-home --shell /usr/sbin/nologin "$LOGWATCHER_USER"
    echo "   User '$LOGWATCHER_USER' created."
else
    echo "   User '$LOGWATCHER_USER' already exists."
fi

# 2. Set the determined installation directory as WORKDIR
WORKDIR="$INSTALL_DIR"
echo "2. Setting installation directory (WORKDIR) to: $WORKDIR"

# --- Functions for Binary Handling ---

check_build_dependencies() {
    echo "   Checking build dependencies: rustc, cargo, build-essential, pkg-config, libssl-dev, openssl..."
    local missing_deps=""
    command -v rustc &>/dev/null || missing_deps+=" rustc"
    command -v cargo &>/dev/null || missing_deps+=" cargo"
    dpkg -s build-essential &>/dev/null || missing_deps+=" build-essential"
    dpkg -s pkg-config &>/dev/null || missing_deps+=" pkg-config"
    dpkg -s libssl-dev &>/dev/null || missing_deps+=" libssl-dev"
    command -v openssl &>/dev/null || missing_deps+=" openssl"

    if [ -n "$missing_deps" ]; then
        echo "   Error: Missing build dependencies:$missing_deps"
        echo "   Please install them using: sudo apt update && sudo apt install$missing_deps"
        exit 1
    fi
    echo "   All build dependencies are installed."
}

build_nightwatcher() {
    echo "   Building '$NIGHTWATCHER_BINARY_NAME' from source in '$SCRIPT_DIR' (as current user)..."
    pushd "$SCRIPT_DIR" > /dev/null
    cargo build --release # Build as current user
    popd > /dev/null
    
    LOCAL_BUILT_BINARY="$SCRIPT_DIR/target/release/$NIGHTWATCHER_BINARY_NAME"
    if [ ! -f "$LOCAL_BUILT_BINARY" ]; then
        echo "   Error: Build failed. Binary not found at '$LOCAL_BUILT_BINARY'."
        exit 1
    fi
    echo "   Binary built successfully at: $LOCAL_BUILT_BINARY"

    # Copy the built binary to the designated INSTALL_DIR with sudo
    echo "   Copying binary to '$INSTALL_DIR' with sudo..."
    sudo cp "$LOCAL_BUILT_BINARY" "$INSTALL_DIR/$NIGHTWATCHER_BINARY_NAME"
    sudo chmod +x "$INSTALL_DIR/$NIGHTWATCHER_BINARY_NAME" # Ensure executable permissions
    echo "   Binary copied to: $INSTALL_DIR/$NIGHTWATCHER_BINARY_NAME"
}

download_nightwatcher() {
    local target_url="$1"
    local binary_filename=$(basename "$target_url")

    echo "   Downloading pre-built '$NIGHTWATCHER_BINARY_NAME' from '$target_url' to '$INSTALL_DIR'..."
    
    if command -v wget &>/dev/null; then
        sudo wget -O "$INSTALL_DIR/$binary_filename" "$target_url"
    elif command -v curl &>/dev/null; then
        sudo curl -L -o "$INSTALL_DIR/$binary_filename" "$target_url"
    else
        echo "   Error: Neither wget nor curl found. Cannot download binary."
        exit 1
    fi

    if [ ! -f "$INSTALL_DIR/$binary_filename" ]; then
        echo "   Error: Download failed. Binary not found at '$INSTALL_DIR/$binary_filename'."
        exit 1
    fi
    sudo chmod +x "$INSTALL_DIR/$binary_filename"
    echo "   Binary downloaded and made executable at: $INSTALL_DIR/$binary_filename"

    # Set the NIGHTWATCHER_PATH after successful download
    NIGHTWATCHER_PATH="$INSTALL_DIR/$binary_filename"

    # --- Integrity Check ---
    echo "   Verifying binary integrity..."
    local_sha_file="$INSTALL_DIR/prebuilt.sha256"

    echo "   Downloading checksum file from $SHA256SUMS_URL..."
    if command -v wget &>/dev/null; then
        sudo wget -O "$local_sha_file" "$SHA256SUMS_URL"
    elif command -v curl &>/dev/null; then
        sudo curl -L -o "$local_sha_file" "$SHA256SUMS_URL"
    else
        echo "   Error: Neither wget nor curl found. Cannot download checksum file for integrity check."
        sudo rm -f "$INSTALL_DIR/$binary_filename" # Clean up potentially unverified binary
        exit 1
    fi
    
    if [ ! -f "$local_sha_file" ]; then
        echo "   Error: Checksum file not downloaded. Cannot verify integrity."
        sudo rm -f "$INSTALL_DIR/$binary_filename" # Clean up potentially unverified binary
        exit 1
    fi

    # Read expected SHA for the specific binary_filename from the combined checksum file
    EXPECTED_SHA=$(grep -E "$binary_filename$" "$local_sha_file" | awk '{print $1}' || true)
    ACTUAL_SHA=$(sha256sum "$NIGHTWATCHER_PATH" | awk '{print $1}')

    if [ -z "$EXPECTED_SHA" ]; then
        echo "   Warning: No SHA256 entry found for '$binary_filename' in '$local_sha_file'."
        echo "   Proceeding without integrity verification. This is a security risk."
    elif [ "$ACTUAL_SHA" != "$EXPECTED_SHA" ]; then
        echo "   Error: Checksum mismatch for $binary_filename!"
        echo "   Expected: $EXPECTED_SHA"
        echo "   Actual:   $ACTUAL_SHA"
        echo "   Download might be corrupted or tampered with. Aborting."
        sudo rm -f "$NIGHTWATCHER_PATH" "$local_sha_file" # Clean up
        exit 1
    else
        echo "   Binary integrity verified successfully."
    fi
    sudo rm -f "$local_sha_file" # Clean up the checksum file after verification
}

# --- Handle Binary Build or Download ---
# Initialize NIGHTWATCHER_PATH. It will be updated by build_nightwatcher or download_nightwatcher.
NIGHTWATCHER_PATH="$WORKDIR/$NIGHTWATCHER_BINARY_NAME"

RAW_ARCH=$(uname -m)
RESOLVED_ARCH="" # This will be the key used for MOCK_RELEASE_URLS lookup

case "$RAW_ARCH" in
    "x86_64" | "amd64")
        RESOLVED_ARCH="x86_64"
        ;;
    "aarch64")
        RESOLVED_ARCH="aarch64"
        ;;
    *)
        echo "Error: Unsupported architecture: $RAW_ARCH."
        echo "This script only supports x86_64 (amd64), i386 (i686), armv7l, and aarch64."
        exit 1
        ;;
esac

TARGET_DOWNLOAD_URL=${MOCK_RELEASE_URLS[$RESOLVED_ARCH]}

if "$BUILD_REQUESTED"; then
    check_build_dependencies
    build_nightwatcher # This function now handles building and sudo-copying
elif [ -n "$TARGET_DOWNLOAD_URL" ]; then
    echo "   Pre-built binary found for architecture '$RAW_ARCH' (resolved to '$RESOLVED_ARCH' for download)."
    download_nightwatcher "$TARGET_DOWNLOAD_URL"
    # NIGHTWATCHER_PATH is set inside download_nightwatcher now
else
    echo "   No pre-built binary URL configured for architecture '$RAW_ARCH' ($RESOLVED_ARCH)."
    echo "   Attempting to build '$NIGHTWATCHER_BINARY_NAME' from source."
    check_build_dependencies
    build_nightwatcher
fi

# Ensure NIGHTWATCHER_PATH is valid after build/download
if [ ! -f "$NIGHTWATCHER_PATH" ]; then
    echo "   Error: '$NIGHTWATCHER_BINARY_NAME' binary not found after operation. Aborting."
    exit 1
fi
echo "   Nightwatcher binary resolved to: $NIGHTWATCHER_PATH"


# 3. Set Permissions for Install Directory, Binary, and Log Files
echo "3. Setting permissions for '$LOGWATCHER_USER' and installation directory..."

# Ensure the INSTALL_DIR exists and grant permissions via ACLs
sudo mkdir -p "$INSTALL_DIR" # Ensure directory exists
sudo setfacl -m u:"$LOGWATCHER_USER":rwx "$INSTALL_DIR" || true
sudo setfacl -m d:u:"$LOGWATCHER_USER":rwx "$INSTALL_DIR" || true # Default ACL for new files

echo "   Permissions for '$LOGWATCHER_USER' on '$INSTALL_DIR' set via ACLs (ownership unchanged)."

# Copy .env file to the install directory
if [ -f "$SCRIPT_DIR/.env" ]; then
    echo "   Copying .env file to '$INSTALL_DIR'..."
    sudo cp "$SCRIPT_DIR/.env" "$INSTALL_DIR/.env"
    sudo setfacl -m u:"$LOGWATCHER_USER":r "$INSTALL_DIR/.env" || true # Ensure logwatcher can read .env
else
    echo "   Warning: .env file not found in script directory. Please ensure it is created in $INSTALL_DIR."
fi


sudo chmod +x "$NIGHTWATCHER_PATH" || true # Ensure the binary is executable for all
sudo setfacl -m u:"$LOGWATCHER_USER":r-x "$NIGHTWATCHER_PATH" || true # Explicit execute for logwatcher via ACL.

# Read LOG_PATH from the .env file in the script's directory for ACL setting
# before it's moved/copied to INSTALL_DIR.
MAIN_LOG_PATH_FROM_ENV=$(grep -E '^LOG_PATH=' "$SCRIPT_DIR/.env" | cut -d '=' -f2- | head -n 1 || true)
if [ -z "$MAIN_LOG_PATH_FROM_ENV" ]; then
    MAIN_LOG_PATH="/var/log/auth.log" # Fallback if LOG_PATH not in .env
    echo "   Warning: LOG_PATH not found in .env. Defaulting to $MAIN_LOG_PATH for ACL."
else
    MAIN_LOG_PATH="$MAIN_LOG_PATH_FROM_ENV"
fi


if [ -f "$MAIN_LOG_PATH" ]; then
    echo "   Setting read permissions for '$LOGWATCHER_USER' on '$MAIN_LOG_PATH'..."
    sudo setfacl -m u:"$LOGWATCHER_USER":r "$MAIN_LOG_PATH" || true
else
    echo "   Warning: Main log file '$MAIN_LOG_PATH' not found. Skipping ACL for it."
fi

COMMAND_LOG_FILE="$INSTALL_DIR/commands.log"
echo "   Setting read/write permissions for '$LOGWATCHER_USER' on '$COMMAND_LOG_FILE' and making it world-readable..."
sudo touch "$COMMAND_LOG_FILE" || true
sudo setfacl -m u:"$LOGWATCHER_USER":rw "$COMMAND_LOG_FILE" || true # Ensure logwatcher can write
sudo chmod a+r "$COMMAND_LOG_FILE" || true # Make it world-readable


# 4. Define Nightwatcher Service Content
SERVICE_CONTENT="
[Unit]
Description=Nightwatcher - Auth Log Telegram Notifier
After=network.target

[Service]
User=$LOGWATCHER_USER
Group=$LOGWATCHER_USER
WorkingDirectory=$INSTALL_DIR
ExecStart=$NIGHTWATCHER_PATH
EnvironmentFile=$INSTALL_DIR/.env
Restart=always
RestartSec=5s
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
"

# 5. Generate/Append the nightwatcher.service file
echo "4. Creating or updating systemd service file: $SERVICE_FILE"
echo "$SERVICE_CONTENT" | sudo tee "$SERVICE_FILE" > /dev/null
echo "   Service file '$SERVICE_FILE' updated."

# 6. Reload Systemd Daemon and Manage Service
echo "5. Reloading systemd, enabling, and starting the service..."
sudo systemctl daemon-reload
sudo systemctl enable nightwatcher.service
sudo systemctl start nightwatcher.service

echo "   Nightwatcher service status:"
sudo systemctl status nightwatcher.service --no-pager || true

echo "--- Nightwatcher Service Setup Complete! ---"
echo "Ensure your .env file containing LOG_PATH, TELEGRAM_BOT_TOKEN, etc., is in: $INSTALL_DIR"
echo "Check logs with: journalctl -u nightwatcher.service -f"
