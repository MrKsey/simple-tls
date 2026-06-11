# Release Instructions

## Creating a Release on GitHub

### Option 1: Using GitHub Web Interface

1. Go to https://github.com/MrKsey/simple-tls/releases/new

2. **Choose version tag:**
   - Click "Choose a tag"
   - Enter `v1.0.0` (or your version number)
   - Click "Create new tag: v1.0.0 on publish"

3. **Target:** `master`

4. **Release title:** `v1.0.0 - Optimized for MIPS/ARM Routers`

5. **Description:** Copy from below section

6. **Attach binaries:**
   - Drag and drop all files from `build/` folder
   - Or upload `simple-tls-v1.0.0-binaries.zip`

7. Check "Set as the latest release"

8. Click "Publish release"

---

### Release Description (copy this):

```markdown
## Optimizations for MIPS/ARM Routers

This release includes major optimizations for MIPS processors (Keenetic and other routers):

### Changes
- ✅ Removed `math/rand` dependency (reduces CPU load on MIPS without FPU)
- ✅ Fixed 8KB buffer allocated once (instead of random 4-8KB per iteration)
- ✅ Optimized deadline updates (every timeout/2 instead of every loop)
- ✅ Reduced memory fragmentation
- ✅ Lower CPU usage on embedded devices

### Available Binaries

| Platform | File | Size |
|----------|------|------|
| Linux ARM64 | `simple-tls-linux-arm64` | 10.44 MB |
| Linux AMD64 | `simple-tls-linux-amd64` | 11.11 MB |
| Linux MIPS LE (Keenetic) | `simple-tls-linux-mipsle-softfloat` | 12.06 MB |
| Linux MIPS BE | `simple-tls-linux-mips-softfloat` | 12.06 MB |
| Windows AMD64 | `simple-tls-windows-amd64.exe` | 11.44 MB |
| Windows ARM64 | `simple-tls-windows-arm64.exe` | 10.51 MB |

### For Keenetic Routers

1. Check architecture: `uname -m`
2. Download `simple-tls-linux-mipsle-softfloat` (most common)
3. Upload to router: `scp simple-tls-linux-mipsle-softfloat root@192.168.1.1:/opt/bin/simple-tls`
4. Make executable: `chmod +x /opt/bin/simple-tls`
5. Run: `/opt/bin/simple-tls -v`

### Building from Source

```bash
# All platforms
GOOS=linux GOARCH=mipsle GOMIPS=softfloat go build -ldflags="-s -w" -o simple-tls .

# See build.ps1 for cross-compilation script
.\build.ps1 all
```

### Documentation

- [MIPS_OPTIMIZATION.md](MIPS_OPTIMIZATION.md) - Detailed optimization info
- [README.md](README.md) - Usage instructions

---

## Previous Versions

This is a beta release. Compatibility between versions is not guaranteed.
```

---

### Option 2: Using GitHub CLI (if installed)

```powershell
# Navigate to project root
cd simple-tls

# Create and publish release
gh release create v1.0.0 `
  --title "v1.0.0 - Optimized for MIPS/ARM Routers" `
  --notes-file RELEASE_INSTRUCTIONS.md `
  build/simple-tls-linux-amd64 `
  build/simple-tls-linux-arm64 `
  build/simple-tls-linux-mipsle-softfloat `
  build/simple-tls-linux-mips-softfloat `
  build/simple-tls-windows-amd64.exe `
  build/simple-tls-windows-arm64.exe
```

---

### After Release

1. Verify release at: https://github.com/MrKsey/simple-tls/releases
2. Update MIPS_OPTIMIZATION.md with release link
3. Announce in relevant communities (optional)
