# Backup And Restore

The CLI backs up the server root containing the Turso database and CAS directory:

```sh
cargo run -p quarry -- server backup /path/to/backup
cargo run -p quarry -- server restore /path/to/backup
```

For a consistent backup, stop the daemon first or run backup when no writes are active. The phase-one command copies the server root directly, including the Turso database, lock-free metadata files, and CAS objects.

The database contains the full canonical IP address recorded when each new
anonymous tmp document was created in a trusted edge deployment. Creation IPs
are operator-only abuse-protection data: they are not returned by Quarry's HTTP
API, but they remain in database copies, EBS snapshots, and other backups for as
long as those artifacts are retained. Protect and expire backups accordingly.
