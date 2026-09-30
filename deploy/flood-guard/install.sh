#!/bin/bash
# Installs the XMUHub flood guard: rules at boot (after the firewall), and the ban check every minute.
set -u
systemctl stop ddos-revert.timer ddos-revert.service 2>/dev/null
command -v conntrack >/dev/null || (yum install -y -q conntrack-tools || dnf install -y -q conntrack-tools || apt-get install -y -qq conntrack) >/dev/null 2>&1
install -m 755 /tmp/xmuhub-ddos-rules /usr/local/sbin/xmuhub-ddos-rules
install -m 755 /tmp/xmuhub-ddos-ban /usr/local/sbin/xmuhub-ddos-ban
cat > /etc/systemd/system/xmuhub-ddos-rules.service <<'EOF'
[Unit]
Description=XMUHub flood guard: per-source connection limit on 80/443, Cloudflare exempt
After=network-online.target firewalld.service
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/xmuhub-ddos-rules

[Install]
WantedBy=multi-user.target
EOF
cat > /etc/systemd/system/xmuhub-ddos-ban.service <<'EOF'
[Unit]
Description=XMUHub flood guard: ban sources holding over 100 connections to 80/443 for an hour

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/xmuhub-ddos-ban
EOF
cat > /etc/systemd/system/xmuhub-ddos-ban.timer <<'EOF'
[Unit]
Description=XMUHub flood guard, every minute

[Timer]
OnBootSec=30
OnUnitActiveSec=60
AccuracySec=5

[Install]
WantedBy=timers.target
EOF
systemctl daemon-reload
systemctl enable --now xmuhub-ddos-rules.service >/dev/null 2>&1
systemctl enable --now xmuhub-ddos-ban.timer >/dev/null 2>&1
/usr/local/sbin/xmuhub-ddos-ban
sleep 5
echo "rules: $(systemctl is-enabled xmuhub-ddos-rules.service) / timer: $(systemctl is-active xmuhub-ddos-ban.timer) / revert: $(systemctl is-active ddos-revert.timer)"
echo "sysctl: $(sysctl -n net.netfilter.nf_conntrack_max) max, established timeout $(sysctl -n net.netfilter.nf_conntrack_tcp_timeout_established)s"
iptables -S INPUT | sed -n 2,3p
iptables -t raw -S PREROUTING | sed -n 2p
echo "banned now: $(ipset list ddos_ban | grep -c '^[0-9]') (v6: $(ipset list ddos_ban6 | grep -c '^[0-9a-f]*:'))"
echo "conntrack: $(cat /proc/sys/net/netfilter/nf_conntrack_count)"
echo "80/443 established: $(ss -Htn state established '( sport = :80 or sport = :443 )' | wc -l)"
curl -s -o /dev/null -w "local app %{http_code} %{time_total}s\n" --max-time 10 http://127.0.0.1:8089/api/meta
