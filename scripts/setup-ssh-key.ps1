# Creates an SSH key on this PC (if needed) and installs it on the remote machine,
# so Nexus can connect without a password. You type the remote password once, here.
param(
  [Parameter(Mandatory)] [string]$HostName,
  [string]$User = '',
  [int]$Port = 22
)

if ($HostName.StartsWith('-') -or $User.StartsWith('-')) { Write-Host "  ✗ Host and user must not start with '-'" -ForegroundColor Red; return }
$target = if ($User) { "$User@$HostName" } else { $HostName }
Write-Host ''
Write-Host "  Nexus  ·  SSH key setup for $target" -ForegroundColor Magenta
Write-Host ''

$sshDir = Join-Path $HOME '.ssh'
$key = Join-Path $sshDir 'id_ed25519'
if (-not (Test-Path $sshDir)) { New-Item -ItemType Directory $sshDir | Out-Null }

if (Test-Path $key) {
  Write-Host "  ✓ Using existing key $key" -ForegroundColor Green
} else {
  Write-Host "  • Creating a new ed25519 key at $key" -ForegroundColor Cyan
  Start-Process ssh-keygen -ArgumentList "-t ed25519 -q -f `"$key`" -N `"`" -C nexus@$env:COMPUTERNAME" -NoNewWindow -Wait
  if (-not (Test-Path $key)) { Write-Host '  ✗ ssh-keygen failed' -ForegroundColor Red; return }
  Write-Host '  ✓ Key created' -ForegroundColor Green
}

$pub = (Get-Content "$key.pub" -Raw).Trim()
Write-Host ''
Write-Host "  • Copying the public key to $target - enter the remote password when asked." -ForegroundColor Cyan
Write-Host ''
$remote = "umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && (grep -qxF '$pub' ~/.ssh/authorized_keys || echo '$pub' >> ~/.ssh/authorized_keys)"
ssh -p $Port -o StrictHostKeyChecking=accept-new -- $target $remote
if ($LASTEXITCODE -ne 0) { Write-Host ''; Write-Host '  ✗ Could not install the key (wrong password, or sshd not running on the remote?)' -ForegroundColor Red; return }

Write-Host ''
Write-Host '  • Testing passwordless login…' -ForegroundColor Cyan
# Offer this key explicitly: an IdentityFile for this host in ~/.ssh/config (e.g. NVIDIA Sync) would
# otherwise stop ssh from trying the default key at all.
$out = ssh -p $Port -i $key -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=6 -- $target 'echo nexus-ok'
if ($out -match 'nexus-ok') {
  Write-Host ''
  Write-Host '  ✓ All set. Close this tab (Ctrl+Shift+W) and click Connect on the machine in Nexus.' -ForegroundColor Green
} else {
  Write-Host '  ✗ Key login still fails. Check that ~/.ssh on the remote is not group/world writable.' -ForegroundColor Red
}
Write-Host ''
