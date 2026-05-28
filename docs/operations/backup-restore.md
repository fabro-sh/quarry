# Backup And Restore

The CLI backs up the server root containing the Turso database and CAS directory:

```sh
cargo run -p quarry -- backup /path/to/backup
cargo run -p quarry -- restore /path/to/backup
```

For a consistent backup, stop the daemon first or run backup when no writes are active. The phase-one command copies the server root directly, including the Turso database, lock-free metadata files, and CAS objects.
