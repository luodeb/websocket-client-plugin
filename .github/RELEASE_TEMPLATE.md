# DeepSeek Plugin Release Template

## Release Checklist

Before creating a release, ensure:

- [ ] All tests pass locally (`./scripts/test-build.sh`)
- [ ] Version in `Cargo.toml` is updated
- [ ] CHANGELOG.md is updated (if exists)
- [ ] All changes are committed and pushed
- [ ] CI builds are passing

## Creating a Release

### Method 1: Git Tag (Recommended)
```bash
# Update version in Cargo.toml first
git add Cargo.toml
git commit -m "Bump version to v1.0.0"
git push

# Create and push tag
git tag v1.0.0
git push origin v1.0.0
```

### Method 2: Manual Dispatch
1. Go to [Actions](../../actions/workflows/release.yml)
2. Click "Run workflow"
3. Enter version (e.g., `v1.0.0`)
4. Select if it's a pre-release
5. Click "Run workflow"

## Release Assets

Each release automatically includes:

- **Linux**: `deepseek-plugin-x86_64.so`
- **Windows**: `deepseek-plugin-x86_64.dll`
- **macOS**: `deepseek-plugin-universal.dylib` (Universal binary for Intel + Apple Silicon)

## Installation Instructions for Users

1. Download the appropriate library for your platform from the [releases page](../../releases)
2. Place the library file in your plugin directory
3. Configure your DeepSeek API key in the plugin settings
4. Restart your application

## Troubleshooting

### Build Failures
- Check that all dependencies are properly specified in `Cargo.toml`
- Ensure the code compiles on all target platforms
- Verify that the plugin interface is correctly implemented

### Release Upload Failures
- Check that the `GITHUB_TOKEN` has sufficient permissions
- Verify that the release was created successfully
- Check the GitHub Actions logs for detailed error messages
