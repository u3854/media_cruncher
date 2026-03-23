#!/bin/bash

# Exit immediately if any command fails
set -e

echo "🦀 Compiling media_cruncher for release (this might take a moment)..."
cargo build --release

echo "📂 Checking for $HOME/.local/bin..."
mkdir -p "$HOME/.local/bin"

echo "🚚 Moving binary to your local path..."
mv target/release/media-cruncher "$HOME/.local/bin/"

echo "✅ Installation complete!"
echo "   You can now run 'media-cruncher' from any terminal."