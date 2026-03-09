# pbuild

A small, parallel build system written in Rust. Built as a cleaner alternative to Makefiles for projects that just need a task runner with incremental builds.

Rules are declared in a `pbuild.toml` file. pbuild hashes input files to decide what needs rebuilding, runs independent rules in parallel, and persists hashes to `.pbuild.lock` for fast incremental builds.

---

## Install

```sh
cargo install --path .
```

---

## Quick start

Run `pbuild init` in your project root to generate a starter `pbuild.toml`:

```sh
pbuild init           # generic starter template
pbuild init --detect  # auto-detect project type (Rust, Python, Node, C, ...)
```

Or write one by hand:

```toml
[config]
default = "app"

["main.o"]
command = ["cc", "-c", "main.c", "-o", "main.o"]
inputs  = ["main.c"]
output  = "main.o"

[app]
command = ["cc", "-o", "app", "main.o"]
deps    = ["main.o"]
inputs  = ["main.o"]
output  = "app"
```

Then run:

```sh
pbuild              # build the default target
pbuild app          # build a specific target
pbuild fmt lint     # build multiple targets in sequence
pbuild --dry-run    # print commands without running them
pbuild --list       # list all targets grouped by category
pbuild status       # show which targets are dirty
pbuild why app      # explain why app would rebuild
pbuild graph        # print the dependency tree
pbuild clean        # delete outputs and reset the lock file
pbuild --version    # print version and build date
```

---

## pbuild.toml reference

```toml
[config]
default    = "build"           # target to build when none is specified
jobs       = 8                 # parallelism (overridden by -j on the CLI)
env        = ["CC", "CFLAGS"] # env vars that trigger a full rebuild when changed
trust      = true              # allow commands that touch system paths (sudo, etc.)
keep_going = true              # keep building independent targets after a failure
max_time   = "10m"             # default timeout for all rules (overridden per-rule)

[vars]
cargo  = "cargo"               # reusable values, interpolated with {{name}}
python = [".venv/bin/python3", "python3"]  # fallback array: first found on PATH wins
solver = { eval = "cargo build-bin app" }  # eval: run a command at load time

[ui]
color  = true                  # force color on/off (default: auto-detect TTY)
prefix = "›"                   # symbol printed before each target name

[build]
group       = "Build"          # group heading in --list output
description = "Build the project"
type        = "task"           # "task" (phony) or "file" (produces output)
command     = ["{{cargo}}", "build", "--release"]
deps        = ["gen"]          # targets that must be built first
inputs      = ["src/**/*.rs"]  # files to hash for dirty-checking (globs ok)
output      = "target/app"     # file produced; hashed after success
depfile     = "main.d"         # compiler depfile; pbuild injects -MF automatically
shell       = true             # run via sh -c (enables pipes, &&, redirects, globs)
dir         = "subdir"         # working directory for this rule
tty         = true             # connect stdin/stdout/stderr directly to the terminal
env         = { RUST_LOG = "debug" }  # extra env vars for this rule only
max_time    = "5m"             # kill the process if it runs longer than this
retry       = 2                # retry on failure this many times (not on timeout)
on_failure  = ["sh", "-c", "rm -f partial.*"]  # run after all retries exhausted
commands    = [                # multiple sequential steps (alternative to command)
    ["cargo", "build"],
    ["cargo", "test"],
]
cache       = false            # always re-run (default: true)
for_each    = "test/*.txt"     # run commands once per matching file ({{file}} substituted)
progress    = "mute"           # "display" (default), "mute", or "percent"

[[build.downloads]]            # download + extract before running commands
url    = "https://example.com/data.tar.gz"
dest   = "vendor/data"
strip  = 1                     # like tar --strip-components
```

### `[config]`

| Key | Type | Description |
|-----|------|-------------|
| `default` | string | Target to build when none is given on the CLI |
| `jobs` | integer | Default parallelism; overridden by `-j` |
| `env` | string array | Env vars that trigger a full rebuild when their value changes |
| `trust` | bool | Allow commands that touch system paths or use sudo (default: false) |
| `keep_going` | bool | Continue building independent targets after a failure (default: false) |
| `max_time` | string | Default timeout for all rules; overridden per-rule |

### `[vars]`

Define reusable values interpolated into commands with `{{name}}`:

```toml
[vars]
cargo  = "cargo"
python = ".venv/bin/python"

[test]
type    = "task"
command = ["{{cargo}}", "test"]
```

If a var is not found in `[vars]`, pbuild falls back to the environment.

**Fallback arrays** let you list candidates in priority order — pbuild picks the first one that resolves to an executable on `PATH`:

```toml
[vars]
python  = [".venv/bin/python3", ".venv/bin/python", "python3"]
maturin = [".venv/bin/maturin", "maturin"]
```

**Eval vars** resolve a shell command at load time and use the output as the value:

```toml
[vars]
solver = { eval = "cabal list-bin cdcl-sat" }
commit = { eval = "git rev-parse --short HEAD" }
```

### `[ui]`

| Key | Type | Description |
|-----|------|-------------|
| `color` | bool | Force color on/off; default auto-detects TTY |
| `prefix` | string | Symbol printed before each target name (default `›`) |

### Named profiles

Profiles let you switch config presets with `-p`:

```toml
[config.profiles.ci]
jobs  = 2
env   = ["CI"]
vars  = { cargo = "cargo" }
```

```sh
pbuild -p ci
```

A profile named `default` is applied automatically on every run.

### Rule fields

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | `"file"` (default) or `"task"` |
| `command` | string array | Single command to run |
| `commands` | array of arrays | Multiple sequential commands; stops on first failure |
| `deps` | string array | Targets that must be built first |
| `inputs` | string array | Files to hash for dirty-checking; globs supported |
| `output` | string | File produced by this rule; hashed after success |
| `depfile` | string | Path where the compiler writes a depfile; pbuild injects `-MF` |
| `shell` | bool | Run via `sh -c` — enables pipes, `&&`, redirects, globs |
| `dir` | string | Working directory to run the command in |
| `subdir` | string | Run `pbuild` (or `make` if no pbuild.toml) in a subdirectory |
| `makedir` | string | Run `make` in a subdirectory |
| `tty` | bool | Connect stdin/stdout/stderr directly to the terminal |
| `env` | table | Extra environment variables set only for this rule |
| `description` | string | Short description shown in `--list` output |
| `group` | string | Group heading in `--list` output |
| `for_each` | string | Glob pattern: run commands once per matching file, substituting `{{file}}` |
| `progress` | string | Output mode: `"display"` (default), `"mute"`, or `"percent"` |
| `downloads` | array of tables | Files to download and extract before running commands |
| `cache` | bool | Set `false` to always re-run this rule (default: `true`) |
| `max_time` | string | Kill the process if it runs longer than this (e.g. `"5m"`, `"30s"`, `"1h"`) |
| `retry` | integer | Number of times to retry on failure, not on timeout (default: `0`) |
| `on_failure` | string array | Command to run after all retries are exhausted (raw argv) |

### `type`

- `file` — the rule produces an output file. pbuild hashes it after success and skips it next run if nothing changed.
- `task` — a phony target (like `test` or `lint`). Always runs if any dep was rebuilt, or if it has no inputs.

### `inputs` and dirty checking

pbuild hashes every file listed in `inputs` before deciding whether to run a rule. If all hashes match `.pbuild.lock`, the rule is skipped. Glob patterns (`src/**/*.rs`, `include/*.h`) are expanded at load time.

A rule with no `inputs` always runs.

### `depfile` — automatic header tracking

When `depfile = "foo.d"` is set, pbuild appends `-MF foo.d` to the command automatically. After the build it parses the depfile and stores every discovered dependency in `.pbuild.lock`.

```toml
["main.o"]
command = ["cc", "-c", "main.c", "-o", "main.o"]
inputs  = ["main.c"]
output  = "main.o"
depfile = "main.d"
```

### `max_time` — timeouts

Kill a hanging process automatically. Accepts `"5m"`, `"30s"`, `"1h"`, `"1h30m"`, or a plain integer (seconds). A global default can be set in `[config]` and overridden per-rule:

```toml
[config]
max_time = "10m"   # default for all rules

[test]
type     = "task"
command  = ["cargo", "test"]
max_time = "5m"    # overrides the global default
```

If a process is killed by timeout, pbuild prints `✗ test  timed out after 5m` and exits with code 3.

### `retry` — automatic retries

Retry a failing rule up to N times before giving up. Timeouts are never retried. A retry banner is printed between attempts:

```toml
[fetch]
type    = "task"
command = ["curl", "-fL", "https://example.com/data.tar.gz", "-o", "data.tar.gz"]
retry   = 3
```

```
› fetch
  ✗ fetch
  ↻ fetch  (attempt 2/4)
  ✓ fetch  1.23s
```

### `on_failure` — cleanup after failure

Run a cleanup command after all retries are exhausted. Written as raw argv — use `["sh", "-c", "..."]` for shell features:

```toml
[build]
type       = "task"
command    = ["cc", "-o", "app", "main.c"]
on_failure = ["sh", "-c", "rm -f app"]
```

### `keep_going`

By default pbuild stops as soon as any rule fails. Set `keep_going = true` in `[config]` or pass `-k` on the CLI to continue building independent targets:

```toml
[config]
keep_going = true
```

```sh
pbuild -k
```

### `shell = true`

Wraps the command in `sh -c`, enabling shell features:

```toml
[bundle]
type    = "task"
shell   = true
command = ["cp -r dist/* build/ && gzip build/app"]
```

### `commands` — multi-step rules

Run multiple commands in sequence. pbuild stops and fails on the first error:

```toml
[ci]
type     = "task"
commands = [
    ["cargo", "fmt", "--check"],
    ["cargo", "clippy", "--", "-D", "warnings"],
    ["cargo", "test"],
]
```

### `tty = true`

Connects stdin, stdout, and stderr directly to the terminal. Use this for interactive programs or anything that needs a real TTY (e.g. QEMU serial console, a REPL):

```toml
[console]
type    = "task"
tty     = true
command = ["qemu-system-riscv64", "-nographic", "-kernel", "kernel.elf"]
```

### `env` — per-rule environment

```toml
[test]
type    = "task"
command = ["cargo", "test"]
env     = { RUST_LOG = "debug", RUST_BACKTRACE = "1" }
```

### `dir` — per-rule working directory

```toml
[build-frontend]
type    = "task"
dir     = "frontend"
command = ["npm", "run", "build"]
```

### `subdir` — nested builds

Run `pbuild` in a subdirectory. If that directory has no `pbuild.toml`, pbuild falls back to `make`:

```toml
[software]
type   = "task"
subdir = "software"
```

Use `makedir` to always invoke `make`:

```toml
[kernel]
type    = "task"
makedir = "software/linux"
```

### `for_each` — run commands per file

Run the rule's commands once for each file matching a glob pattern. `{{file}}` in commands is replaced with each matched path:

```toml
[bench]
type     = "task"
for_each = "bench/uf50/*.cnf"
command  = ["./solver", "{{file}}"]
```

```
› bench
  ✓ bench  (1000 files)  4.21s
```

### `progress` — output mode

- `"display"` (default) — show all output normally
- `"mute"` — suppress command output on success; errors are still shown
- `"percent"` — for `for_each` rules, show a live progress counter

```toml
[bench]
type     = "task"
progress = "percent"
for_each = "bench/*.cnf"
command  = ["./solver", "{{file}}"]
```

```
› bench
    [42/1000] 4%  bench
  ✓ bench  (1000 files)  4.21s
```

### `downloads` — declarative file downloads

Download and extract archives before running a rule's commands. Each download is skipped if `dest/.done` already exists.

```toml
[fetch-data]
type    = "task"
command = ["echo", "data ready"]

[[fetch-data.downloads]]
url   = "https://example.com/dataset.tar.gz"
dest  = "data/training"
strip = 1
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | required | URL to fetch (HTTP/HTTPS) |
| `dest` | string | required | Directory to extract into (created if missing) |
| `extract` | string | auto-detect | Archive format: `tar.gz`, `tgz`, `tar`, or `none` |
| `strip` | integer | `0` | Strip this many leading path components |

### `env` tracking

Variables listed in `[config] env` are stored in `.pbuild.lock`. If any change between runs, every rule rebuilds.

### Output conflict detection

pbuild errors at load time if two rules declare the same `output` file:

```
output conflicts detected:
  output `build/app` claimed by both `debug` and `release`
```

### Safety checks

pbuild refuses to run commands that use privilege escalation (`sudo`, `su`, `doas`), modify system paths, or contain dangerous shell patterns (`rm -rf`, `| sh`, fork bombs).

To allow them, pass `--trust` on the CLI or set `trust = true` in `[config]`.

### `{{args}}` — extra arguments

```toml
[test]
type    = "task"
command = ["cargo", "test", "{{args}}"]
```

```sh
pbuild test -- --nocapture my_test_name
```

---

## CLI reference

```
Usage: pbuild [OPTIONS] [TARGET...] [-- EXTRA_ARGS]

Options:
  -j <N>, --jobs <N>   Run at most N rules in parallel (default: logical CPUs)
  -n, --dry-run        Print commands without running them
  -q, --quiet          Suppress pbuild status lines; show only command output
  -k, --keep-going     Keep building independent rules after a failure
  -v, --verbose        Print skipped rules and dirty reasons
  -l, --list           List all available targets and exit
  -h, --help           Print this help and exit
      --version        Print version (git hash + build date) and exit
  -p, --profile <name> Activate a named profile from [config.profiles.<name>]
  -w, --watch          Rebuild automatically when input files change
      --trust          Skip safety checks for dangerous commands
      --only           Build just the named target without running its dependencies
      --log <file>     Tee output to a file (appends; no ANSI codes)
      --explain        Show fully-expanded commands and env for each target, then exit
      --completion     Print shell completion script (fish, bash, or zsh)
  --                   Pass remaining arguments to the target command

Subcommands:
  init                  Write a starter pbuild.toml in the current directory
  init --detect         Auto-detect project type and scaffold real targets
  import [Makefile]     Convert a Makefile into pbuild.toml (default: Makefile)
  add <name>            Interactively scaffold a new rule in pbuild.toml
  edit [TARGET]         Open pbuild.toml in $EDITOR at the given target's rule
  run <TARGET>          Alias for pbuild <TARGET>
  status [TARGET]       Show which targets are dirty (would rebuild)
  status --json         Emit status as JSON
  clean                 Delete all rule outputs and .pbuild.lock
  clean <TARGET>        Delete one target's output and its lock entries
  touch <TARGET>        Mark a target clean without rebuilding it
  prune                 Remove stale entries from .pbuild.lock
  retry                 Re-run the last failed target
  doctor                Check config health (commands on PATH, globs, deps, output conflicts)
  why <TARGET>          Explain why a target would rebuild
  why --json <TARGET>   Emit why output as JSON
  graph [TARGET]        Print the dependency tree
  graph --dot [TARGET]  Emit Graphviz DOT format
  graph --json [TARGET] Emit dependency graph as JSON
```

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Build failure (a rule's command exited non-zero) |
| `2` | Config error (bad `pbuild.toml`, unknown target, missing dep) |
| `3` | Timeout (a rule was killed by `max_time`) |

### Multi-target builds

```sh
pbuild fmt lint test
```

### `--explain`

Print fully-expanded commands, environment, working directory, timeout, and retry count for each rule in the plan — without running anything:

```sh
pbuild --explain
pbuild --explain test
```

### `pbuild doctor`

Validates your `pbuild.toml` without running any build commands. Checks:
- All commands exist on `PATH`
- All glob patterns in `inputs` match at least one file
- All `deps` reference known targets
- No two rules declare the same `output` file

```sh
pbuild doctor
```

### `pbuild --watch`

Watches all input files in the build plan and rebuilds whenever any of them changes:

```sh
pbuild --watch
pbuild --watch test
pbuild -p ci --watch
```

### `pbuild status [--json]`

Shows which targets are dirty (would rebuild) without running anything.

### `pbuild why <target> [--json]`

Explains exactly why a target would rebuild — lists the first changed input, any rebuilt dep, or any changed tracked env var.

### `pbuild graph [--dot] [--json]`

Prints an ASCII dependency tree, Graphviz DOT, or JSON adjacency list:

```sh
pbuild graph app
pbuild graph --dot app | dot -Tsvg > graph.svg
pbuild graph --json app
```

### `pbuild touch <target>`

Hashes the target's inputs and output as they are now, marking it clean without rebuilding.

### `pbuild retry`

Re-runs the last failed target. The failed target is recorded in `.pbuild.lock`.

### `pbuild import`

Converts a Makefile into `pbuild.toml`:

```sh
pbuild import           # reads ./Makefile
pbuild import GNUmakefile
```

### Shell completions

```sh
# fish
pbuild --completion fish > ~/.config/fish/completions/pbuild.fish

# bash  (~/.bashrc)
eval "$(pbuild --completion bash)"

# zsh
pbuild --completion zsh > "${fpath[1]}/_pbuild"
```

---

## How it works

1. `pbuild.toml` is parsed into a list of rules. Output conflicts are detected immediately.
2. A dependency graph is built and topologically sorted.
3. Rules are executed in waves — all rules whose dependencies are satisfied run in parallel, bounded by `-j`.
4. When only one rule is ready, its output is streamed live. When multiple rules run in parallel, each is buffered and printed atomically to prevent interleaving.
5. After each wave, input and output files are hashed and flushed to `.pbuild.lock`.
6. On the next run, rules whose hashes haven't changed are skipped.
7. If a rule fails, `on_failure` runs, then `retry` attempts are made. With `-k`/`keep_going`, independent rules continue building.

---

## License

MIT — see [LICENSE](LICENSE)
