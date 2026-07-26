# AUR Packaging for Wander 🎵

This directory contains the Arch User Repository (AUR) package specifications for **Wander**.

---

## Package Variants

1. **`PKGBUILD` (`wander`)**: Stable package built from released version tags (e.g. `v0.1.0`).
2. **`PKGBUILD.git` (`wander-git`)**: Development VCS package built from the latest commit on `master`.

---

## 🛠️ How to Publish to the AUR

### Step 1: Set up SSH Access for AUR

If you haven't already added your SSH public key to your AUR account:

1. Generate a dedicated SSH key for AUR:
   ```bash
   ssh-keygen -f ~/.ssh/aur -t ed25519
   ```
2. Configure SSH in `~/.ssh/config`:
   ```sshconfig
   Host aur.archlinux.org
     IdentityFile ~/.ssh/aur
     User aur
   ```
3. Copy `~/.ssh/aur.pub` and add it to your profile settings at [https://aur.archlinux.org/account/](https://aur.archlinux.org/account/).

---

### Step 2: Publish `wander-git` (Development Package)

1. Clone the AUR repository (creates a local workspace):
   ```bash
   git clone ssh://aur@aur.archlinux.org/wander-git.git /tmp/aur-wander-git
   ```

2. Copy `PKGBUILD.git` as `PKGBUILD`:
   ```bash
   cp packaging/aur/PKGBUILD.git /tmp/aur-wander-git/PKGBUILD
   ```

3. Generate `.SRCINFO`:
   ```bash
   cd /tmp/aur-wander-git
   makepkg --printsrcinfo > .SRCINFO
   ```

4. Commit & Push:
   ```bash
   git add PKGBUILD .SRCINFO
   git commit -m "Initial upload: wander-git"
   git push origin master
   ```

---

### Step 3: Publish `wander` (Stable Release Package)

1. Tag a release on your git repository and push it:
   ```bash
   git tag -a v0.1.0 -m "Release v0.1.0"
   git push origin v0.1.0
   ```

2. Clone the stable AUR repository:
   ```bash
   git clone ssh://aur@aur.archlinux.org/wander.git /tmp/aur-wander
   ```

3. Copy `PKGBUILD` and update the SHA256 checksum:
   ```bash
   cp packaging/aur/PKGBUILD /tmp/aur-wander/PKGBUILD
   cd /tmp/aur-wander
   # Calculate sha256 sum of release tarball:
   updpkgsums
   makepkg --printsrcinfo > .SRCINFO
   ```

4. Commit & Push:
   ```bash
   git add PKGBUILD .SRCINFO
   git commit -m "Initial upload: wander 0.1.0"
   git push origin master
   ```

---

## 🔍 Testing Packaging Locally

To test building the package locally using `makepkg` before submitting:

```bash
cd packaging/aur
makepkg -si --needed
```
