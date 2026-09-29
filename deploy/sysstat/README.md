# 服务器流量记录（sysstat）

每分钟记一次网卡、CPU、内存，保留 7 天，用来回看「某个时间是不是带宽满了」。

```sh
sar -n DEV -s 22:00:00 -e 22:50:00          # 今天 22:00–22:50 的网卡流量（rxkB/s、txkB/s）
sar -n DEV -f /var/log/sa/sa29              # 29 号全天
sar -u -s 22:00:00 -e 22:50:00              # CPU
```

安装（已在服务器上做过）：

```sh
mkdir -p /etc/systemd/system/sysstat-collect.timer.d
printf '[Timer]\nOnCalendar=\nOnCalendar=*:*:00\n' > /etc/systemd/system/sysstat-collect.timer.d/every-minute.conf
sed -i 's/^HISTORY=.*/HISTORY=7/' /etc/sysconfig/sysstat
systemctl daemon-reload && systemctl enable --now sysstat sysstat-collect.timer sysstat-summary.timer
```

请求耗时日志见 [../nginx/README.md](../nginx/README.md)。
