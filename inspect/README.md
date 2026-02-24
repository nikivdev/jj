# jj-inspect

Minimal TUI stack review for Jujutsu (jj).

## Run
```bash
cargo run -p jj-inspect
```

Options:
```bash
jj-inspect --repo <path> --base <revset> --limit <n>
jj-inspect --repo <path> --queue
```

## Keys
- `j`/`k` or arrows: move file selection
- `[` / `]`: prev/next commit
- `PgDn`/`PgUp`: scroll diff
- `g`/`G`: top/bottom
- `r`: refresh
- `Enter`: open full diff in pager
- `:`: command mode (run shell command)
- `A`: approve selected commit (queue mode)
- `q`: quit
