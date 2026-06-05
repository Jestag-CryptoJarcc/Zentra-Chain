# Running a Zentra seed node on a VPS

A **seed node** is an always-on Zentra node with a public IP that fresh wallets
connect to automatically. It bootstraps connections and relays blocks between
miners — it does **not** need to mine itself (your PCs do that).

> ## Which method to use?
> - **Small VPS (< 2 GB RAM): use Method A (prebuilt binary + systemd).**
>   Do **not** build from source on a small box — compiling RocksDB needs
>   2–4 GB and will be OOM-killed.
> - Big VPS (≥ 2 GB RAM) and you want full container isolation: Method B (Docker).

You must open **port 16110/tcp** in both the OS firewall and your VPS provider's
firewall/security group.

---

## Method A — Prebuilt binary + systemd (best for small VPS)

This uses ~150–250 MB RAM. Perfect for a 1 GB VPS.

```bash
# 1. Create an isolated folder + user for the node
sudo useradd -r -m -d /opt/zentra zentra 2>/dev/null || true
sudo mkdir -p /opt/zentra/data
cd /opt/zentra

# 2. Download the prebuilt Linux node (no compiling!)
curl -L -o zentra.tar.gz \
  https://github.com/Jestag-CryptoJarcc/Zentra-Chain/releases/latest/download/zentra-linux-x64.tar.gz
tar -xzf zentra.tar.gz
# the daemon is at zentra-linux-x64/bin/zentrad
sudo cp zentra-linux-x64/bin/zentrad /usr/local/bin/zentrad
sudo chmod +x /usr/local/bin/zentrad
sudo chown -R zentra:zentra /opt/zentra
```

Create the service:

```bash
sudo tee /etc/systemd/system/zentra-seed.service >/dev/null <<'EOF'
[Unit]
Description=Zentra Seed Node (devnet relay)
After=network-online.target
Wants=network-online.target

[Service]
User=zentra
Group=zentra
# Relay/seed mode: NO --mine (keeps RAM/CPU low). Your PCs do the mining.
ExecStart=/usr/local/bin/zentrad --network devnet --data-dir /opt/zentra/data
Restart=always
RestartSec=5
# Low-memory safety
MemoryMax=500M
Nice=10

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now zentra-seed
```

Open the firewall:

```bash
sudo ufw allow 16110/tcp        # (also open TCP 16110 in your provider's panel)
```

**Manage it (easy to find back):**
```bash
systemctl status zentra-seed        # running?
journalctl -u zentra-seed -f        # live logs (height, peers)
sudo systemctl restart zentra-seed  # restart
sudo systemctl stop zentra-seed     # stop
```

---

## Method B — Docker (full isolation, needs ≥ 2 GB RAM to build)

```bash
curl -fsSL https://get.docker.com | sh
git clone https://github.com/Jestag-CryptoJarcc/Zentra-Chain.git
cd Zentra-Chain
docker build -t zentra-node .
docker run -d --name zentra-seed --restart unless-stopped \
  -p 16110:16110 -v zentra-data:/data zentra-node
docker logs -f zentra-seed
```

(The bundled `Dockerfile` builds the daemon; on a small VPS build the image on a
bigger machine or use Method A instead.)

---

## Get your public IP (to bake in as the default seed)

```bash
curl -s ifconfig.me
```

Give that IP to set `DEFAULT_SEED_PEERS = ["YOUR_VPS_IP:16110"]` in
`crates/zentrad/src/p2p_sync.rs`, then cut a new release — every downloaded
wallet will then auto-connect to your network with zero config.

Verify it's reachable from your own PC:
```bash
nc -vz YOUR_VPS_IP 16110     # "succeeded" = good
```

---

## Keeping devnet alive

The seed only relays. For the chain to advance, **mine on at least one PC** (the
wallet's Mining tab). Those mined blocks broadcast to the seed, which then serves
them to every other wallet that connects. If you ever want the seed itself to
mine too (so the chain advances even with no PCs on), add `--mine` to the
`ExecStart` line — but on a 1 GB VPS, prefer mining on your PCs.
