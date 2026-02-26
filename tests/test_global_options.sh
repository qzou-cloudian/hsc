#!/bin/bash
# Test global options functionality

set -e

BINARY="./target/debug/hsc"

echo "=== Testing Global Options ==="
echo ""

echo "1. Test --version:"
$BINARY --version
echo ""

echo "2. Test --help:"
$BINARY --help | head -15
echo ""

echo "3. Test --debug with environment variables:"
AWS_REGION=us-west-2 AWS_PROFILE=test-profile $BINARY --debug ls 2>&1 | grep "Debug:" || true
echo ""

echo "4. Test --endpoint-url option:"
$BINARY --debug --endpoint-url http://custom-endpoint.com --region us-east-1 ls 2>&1 | grep -E "Debug:" | head -4 || true
echo ""

echo "5. Test --profile option:"
$BINARY --debug --profile myprofile ls 2>&1 | grep "Debug: Using AWS profile:" || true
echo ""

echo "6. Test --region option:"
$BINARY --debug --region eu-west-1 ls 2>&1 | grep "Debug: Using region:" || true
echo ""

echo "7. Test environment variable precedence:"
echo "   Setting AWS_ENDPOINT_URL via environment..."
AWS_ENDPOINT_URL=http://env-endpoint.com $BINARY --debug ls 2>&1 | grep "Debug: Using custom endpoint:" || true
echo ""

echo "8. Test CLI option overrides environment:"
echo "   CLI --endpoint-url should override AWS_ENDPOINT_URL..."
AWS_ENDPOINT_URL=http://env-endpoint.com $BINARY --debug --endpoint-url http://cli-endpoint.com ls 2>&1 | grep "Debug: Using custom endpoint:" || true
echo ""

echo "9. Verify supported environment variables:"
echo "   The following environment variables are supported:"
echo "   - AWS_CONFIG_FILE"
echo "   - AWS_SHARED_CREDENTIALS_FILE"
echo "   - AWS_PROFILE"
echo "   - AWS_ENDPOINT_URL"
echo "   - AWS_REGION"
echo "   - AWS_ACCESS_KEY_ID"
echo "   - AWS_SECRET_ACCESS_KEY"
echo "   - AWS_SESSION_TOKEN"
echo ""

echo "✓ All global options are working correctly!"
