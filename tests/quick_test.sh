#!/bin/bash
# Quick Test - Minimal test to verify basic functionality

set -e

BINARY="./target/debug/hsc"
CONFIG_FILE="${1:-$HOME/.aws/config}"

if [ ! -f "$BINARY" ]; then
    echo "Building project..."
    cargo build
fi

if [ ! -f "$CONFIG_FILE" ]; then
    echo "Error: Config file not found: $CONFIG_FILE"
    echo "Create a config file with your AWS credentials"
    exit 1
fi

export AWS_CONFIG_FILE="$CONFIG_FILE"
export AWS_SHARED_CREDENTIALS_FILE="$CONFIG_FILE"
export AWS_REGION="us-east-1"

echo "=== Quick Test ==="
echo ""
echo "1. Listing your S3 buckets:"
$BINARY ls
echo ""
echo "2. Showing help for copy command:"
$BINARY cp --help
echo ""
echo "✓ Basic functionality working!"
echo ""
echo "Run './test_s3.sh config' for comprehensive tests"
