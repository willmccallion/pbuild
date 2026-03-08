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
```

---

## pbuild.toml reference

```toml
[config]
default = "build"            # target to build when none is specified
jobs    = 8                  # parallelism (overridden by -j on the CLI)
env     = ["CC", "CFLAGS"]  # env vars that trigger a full rebuild when changed
trust   = true               # allow commands that touch system paths (sudo, etc.)

[vars]
cargo  = "cargo"             # reusable values, interpolated with {{name}}
python = [".venv/bin/python3", "python3"]  # fallback array: first one found on PATH wins

[ui]
color  = true                # force color on/off (default: auto-detect TTY)
prefix = "›"                 # symbol printed before each target name

[build]
group       = "Build"        # group heading in --list output
description = "Build the project"
type        = "task"         # "task" (phony) or "file" (produces output)
command     = ["{{cargo}}", "build", "--release"]
deps        = ["gen"]        # targets that must be built first
inputs      = ["src/**/*.rs"] # files to hash for dirty-checking (globs ok)
output      = "target/app"   # file produced; hashed after success
depfile     = "main.d"       # compiler depfile; pbuild injects -MF automatically
shell       = true           # run via sh -c (enables pipes, &&, redirects, globs)
dir         = "subdir"       # working directory for this rule
tty         = true           # connect stdin/stdout/stderr directly to the terminal
env         = { RUST_LOG = "debug" }  # extra env vars for this rule only
commands    = [              # multiple sequential steps (alternative to command)
    ["cargo", "build"],
    ["cargo", "test"],
]
```

### `[config]`

| Key | Type | Description |
|-----|------|-------------|
| `default` | string | Target to build when none is given on the CLI |
| `jobs` | integer | Default parallelism; overridden by `-j` |
| `env` | string array | Env vars that trigger a full rebuild when their value changes |
| `trust` | bool | Allow commands that touch system paths or use sudo (default: false) |

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
env   = { RUST_BACKTRACE = "1" }
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
| `tty` | bool | Connect stdin/stdout/stderr directly to the terminal (for interactive programs) |
| `env` | table | Extra environment variables set only for this rule |
| `description` | string | Short description shown in `--list` output |
| `group` | string | Group heading in `--list` output |

### `type`

- `file` — the rule produces an output file. pbuild hashes it after success and skips it next run if nothing changed.
- `task` — a phony target (like `test` or `lint`). Always runs if any dep was rebuilt, or if it has no inputs.

### `inputs` and dirty checking

pbuild hashes every file listed in `inputs` before deciding whether to run a rule. If all hashes match `.pbuild.lock`, the rule is skipped. Glob patterns (`src/**/*.rs`, `include/*.h`) are expanded at load time.

A rule with no `inputs` always runs.

### `depfile` — automatic header tracking

When `depfile = "foo.d"` is set, pbuild appends `-MF foo.d` to the command automatically. After the build it parses the depfile and stores every discovered dependency in `.pbuild.lock`. On future runs those files are checked — so modifying any included header triggers a rebuild without listing every header manually.

```toml
["main.o"]
command = ["cc", "-c", "main.c", "-o", "main.o"]
inputs  = ["main.c"]
output  = "main.o"
depfile = "main.d"
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

Set environment variables for a single rule without affecting others:

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

### `env` tracking

Variables listed in `[config] env` are stored in `.pbuild.lock`. If any change between runs, every rule rebuilds — catches the classic mistake of changing `CC` and getting a silently stale binary.

### Safety checks

pbuild refuses to run commands that use privilege escalation (`sudo`, `su`, `doas`), modify system paths (`/etc/`, `/usr/`, `/bin/`), or contain dangerous shell patterns (`rm -rf`, `| sh`, fork bombs).

To allow them, pass `--trust` on the CLI or set `trust = true` in `[config]`:

```toml
[config]
trust = true

[install]
type    = "task"
command = ["sudo", "cp", "app", "/usr/local/bin/app"]
```

### `{{args}}` — extra arguments

Pass arguments from the CLI to a rule with `pbuild target -- arg1 arg2`. If the command contains `{{args}}`, they are inserted there; otherwise they are appended at the end:

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
  -p, --profile <name> Activate a named profile from [config.profiles.<name>]
  -w, --watch          Rebuild automatically when input files change
      --trust          Skip safety checks for dangerous commands
      --only           Build just the named target without running its dependencies
      --log <file>     Tee output to a file (appends; no ANSI codes)
      --completion     Print shell completion script (fish, bash, or zsh)
  --                   Pass remaining arguments to the target command

Subcommands:
  init                 Write a starter pbuild.toml in the current directory
  init --detect        Auto-detect project type and scaffold real targets
  import [Makefile]    Convert a Makefile into pbuild.toml (default: Makefile)
  add <name>           Interactively scaffold a new rule in pbuild.toml
  edit [TARGET]        Open pbuild.toml in $EDITOR at the given target's rule
  run <TARGET>         Alias for pbuild <TARGET>
  status [TARGET]      Show which targets are dirty (would rebuild)
  clean                Delete all rule outputs and .pbuild.lock
  clean <TARGET>       Delete one target's output and its lock entries
  touch <TARGET>       Mark a target clean without rebuilding it
  prune                Remove stale entries from .pbuild.lock
  retry                Re-run the last failed target
  why <TARGET>         Explain why a target would rebuild
  graph [TARGET]       Print the dependency tree
  graph --dot [TARGET] Emit Graphviz DOT format
```

### Multi-target builds

Pass multiple targets and pbuild runs each in order:

```sh
pbuild fmt lint test
```

### `pbuild --watch`

Watches all input files in the build plan and rebuilds whenever any of them changes. Works with `--profile` and all other flags:

```sh
pbuild --watch
pbuild --watch test
pbuild -p ci --watch
```

### `pbuild status`

Shows which targets are dirty (would rebuild) without running anything:

```
  build    dirty
  test     clean
  lint     clean
```

### `pbuild why <target>`

Explains exactly why a target would rebuild — lists the first changed input file, any dep that was rebuilt, or any tracked env var that changed.

### `pbuild graph`

Prints an ASCII dependency tree. `--dot` emits Graphviz DOT format, with missing dependencies highlighted in red:

```sh
pbuild graph app
pbuild graph --dot app | dot -Tsvg > graph.svg
```

### `pbuild touch <target>`

Hashes the target's inputs and output as they are now, writing them to `.pbuild.lock`. The next build will see the target as clean and skip it. Useful after manually editing an output file.

### `pbuild clean <target>`

Deletes one target's output file and removes its entries from `.pbuild.lock`, forcing it to rebuild next run. Without a target, cleans everything.

### `pbuild prune`

Removes stale entries from `.pbuild.lock` — files that are tracked but no longer referenced by any rule. Keeps the lock file tidy over time.

### `pbuild retry`

Re-runs the last failed target. The failed target is recorded in `.pbuild.lock`; a successful build clears it.

```sh
pbuild retry
```

### `pbuild import`

Converts a Makefile into `pbuild.toml`. Handles simple rules, multi-step recipes, variables (converted to `[vars]`), and aggregate targets:

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

1. `pbuild.toml` is parsed into a list of rules.
2. A dependency graph is built and topologically sorted.
3. Rules are executed in waves — all rules whose dependencies are satisfied run in parallel, bounded by `-j`.
4. When only one rule is ready, its output is streamed live. When multiple rules run in parallel, each is buffered and printed atomically to prevent interleaving.
5. After each wave, input and output files are hashed and flushed to `.pbuild.lock`.
6. On the next run, rules whose hashes haven't changed are skipped.

---

## License

MIT — see [LICENSE](LICENSE)
