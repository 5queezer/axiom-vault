# Cloud RAID

AxiomVault Cloud RAID distributes encrypted vault data across multiple storage backends for redundancy and fault tolerance. Two modes are supported:

- **Mirror (RAID 1):** Every chunk is replicated to all backends. Simple, high redundancy.
- **Erasure (RAID 5/6):** Reed-Solomon coding splits each chunk into *k* data shards and *m* parity shards. Tolerates up to *m* backend failures.

## CLI Commands

### Add a backend

```bash
axiomvault raid-add-backend -p /path/to/vault -t local -c '{"root": "/mnt/backup1"}'
axiomvault raid-add-backend -p /path/to/vault -t local -c '{"root": "/mnt/backup2"}'
```

### Configure RAID mode

```bash
# Mirror mode (default)
axiomvault raid-configure -p /path/to/vault --mode mirror

# Erasure mode: 2 data shards + 1 parity shard (requires 3 backends)
axiomvault raid-configure -p /path/to/vault --mode erasure -k 2 -m 1
```

### Check status

```bash
axiomvault raid-status -p /path/to/vault
```

Output shows mode, backend health, shard counts, and redundancy summary.

### Rebuild after failure

```bash
# Auto-detect first degraded backend
axiomvault raid-rebuild -p /path/to/vault

# Target a specific backend
axiomvault raid-rebuild -p /path/to/vault -t 1
```

Displays a live progress bar during the rebuild.

### Remove a backend

```bash
axiomvault raid-remove-backend -p /path/to/vault -i 2
```

In erasure mode, shards are automatically redistributed before removal. In mirror mode, data already exists on the remaining backends.

## Example Workflow: Mirror Mode with Local Backends

```bash
# 1. Create a vault
axiomvault create --name my-vault --path /tmp/vault

# 2. Add two local backends
axiomvault raid-add-backend -p /tmp/vault -t local -c '{"root": "/tmp/backend-a"}'
axiomvault raid-add-backend -p /tmp/vault -t local -c '{"root": "/tmp/backend-b"}'

# 3. Configure mirror mode
axiomvault raid-configure -p /tmp/vault --mode mirror

# 4. Add files (they will be replicated to both backends)
axiomvault add -p /tmp/vault -s ./document.pdf -d /docs/document.pdf

# 5. Check status — both backends should be healthy
axiomvault raid-status -p /tmp/vault

# 6. Simulate failure: delete data from backend-b
rm -rf /tmp/backend-b/*

# 7. Check status — backend 1 should show as degraded
axiomvault raid-status -p /tmp/vault

# 8. Rebuild the failed backend
axiomvault raid-rebuild -p /tmp/vault -t 1

# 9. Verify recovery
axiomvault raid-status -p /tmp/vault
```
