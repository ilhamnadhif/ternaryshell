#!/usr/bin/env bash
# Starts an unprivileged OpenSSH server on 127.0.0.1 for manual testing (key auth only, as the
# current user). Usage: scripts/dev-sshd.sh [dir] [port]   — stop it with: kill $(cat <dir>/sshd.pid)
set -euo pipefail
DIR="${1:-${TMPDIR:-/tmp}/ternaryshell-dev-sshd}"
PORT="${2:-2299}"
SSH_USER="${USER:-$(id -un)}"
mkdir -p -m 700 "$DIR"
chmod 700 "$DIR"
DIR="$(cd "$DIR" && pwd)"
[ -f "$DIR/host_ed25519" ] || ssh-keygen -q -t ed25519 -N "" -f "$DIR/host_ed25519"
[ -f "$DIR/client_ed25519" ] || ssh-keygen -q -t ed25519 -N "" -C ternaryshell-dev -f "$DIR/client_ed25519"
cp "$DIR/client_ed25519.pub" "$DIR/authorized_keys"
chmod 600 "$DIR/authorized_keys"
cat > "$DIR/sshd_config" <<EOF
Port $PORT
ListenAddress 127.0.0.1
AllowUsers "$SSH_USER"
HostKey "$DIR/host_ed25519"
AuthorizedKeysFile "$DIR/authorized_keys"
PidFile "$DIR/sshd.pid"
UsePAM no
# /tmp is world-writable with sticky bit; the subdir and authorized_keys are 0700/0600 above.
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
Subsystem sftp internal-sftp
LogLevel ERROR
AcceptEnv LANG LC_*
EOF
/usr/sbin/sshd -f "$DIR/sshd_config"
echo "sshd on 127.0.0.1:$PORT"
echo "user: $SSH_USER"
echo "key:  $DIR/client_ed25519"
echo "host key: $(cut -d' ' -f1,2 "$DIR/host_ed25519.pub")"
