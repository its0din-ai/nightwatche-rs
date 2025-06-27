# Nightwatche-rs: Building from Source

This guide provides instructions on how to compile the `nightwatche-rs` application from its source code. Building from source allows you to customize the application, contribute to its development, or ensure you have the latest features and bug fixes.

## Prerequisites

Before you begin, ensure your system has the following software installed:

1. **Rust Toolchain:**
   `nightwatche-rs` is built with Rust. You'll need `rustc` (the Rust compiler) and `cargo` (Rust's package manager and build system). The easiest way to install these is by using `rustup`:
   ```curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh```
    Follow the on-screen instructions. After installation, you might need to restart your terminal or run `source $HOME/.cargo/env`.

2. **System Build Dependencies:**
`nightwatche-rs` relies on system libraries, especially for networking and SSL/TLS operations. For Debian/Ubuntu-based systems, install them using:

```sudo apt update```
```sudo apt install -y build-essential pkg-config libssl-dev openssl```


* `build-essential`: Provides essential compilation tools like `gcc`.

* `pkg-config`: Helps `cargo` find installed libraries.

* `libssl-dev`: Development files for OpenSSL, necessary for secure communication.

* `openssl`: The OpenSSL command-line tool, often a dependency or useful for verification.

For other Linux distributions (Fedora, Arch, etc.), the package names might vary (e.g., `openssl-devel` instead of `libssl-dev`).

## Building the Project

Follow these steps to build `nightwatche-rs`:

1. **Clone the Repository:**
If you haven't already, clone the `nightwatche-rs` GitHub repository to your local machine:

```git clone https://github.com/its0din-ai/nightwatche-rs.git```
```cd nightwatche-rs```


2. **Compile the Project:**
Navigate into the cloned directory and use `cargo` to build the project in release mode. The release build is optimized for performance and smaller binary size.

```cargo build --release```


This command will download dependencies, compile your code, and place the resulting executable in the `target/release/` directory.

3. **Find the Executable:**
The compiled binary will be located at:

```./target/release/nightwatche-rs```


## Running / Deployment

After successfully building the binary, you can use the `install.sh` script (if available in your repository) to set up `nightwatche-rs` as a system service.

To use your newly built binary with the `install.sh` script, run it with the `--build` flag from the root of your `nightwatche-rs` project:

```sudo ./install.sh --build```


This command will use the binary you just built in `./target/release/` for deployment.

## Troubleshooting

* **"command not found: cargo" or "command not found: rustc"**: Ensure Rust and Cargo are correctly installed and that your shell's `PATH` environment variable includes `$HOME/.cargo/bin`. You might need to restart your terminal or run `source $HOME/.cargo/env`.

* **Compilation errors related to `openssl` or `ssl`**: Double-check that `libssl-dev` (or its equivalent for your OS) and `pkg-config` are installed.

* **"error: linker `cc` not found"**: This indicates missing C/C++ build tools. Install `build-essential` (on Debian/Ubuntu).

If you encounter persistent issues, please refer to the official Rust documentation or the `nightwatche-rs` repository for more specific troubleshooting.