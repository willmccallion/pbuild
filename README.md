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
pbuild init
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
pbuild --dry-run    # print commands without running them
pbuild --list       # list all targets grouped by category
pbuild status       # show which targets are dirty
pbuild why app      # explain why app would rebuild
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
cargo = "cargo"              # reusable values, interpolated with {{name}}

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

### `[ui]`

| Key | Type | Description |
|-----|------|-------------|
| `color` | bool | Force color on/off; default auto-detects TTY |
| `prefix` | string | Symbol printed before each target name (default `›`) |

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

### `dir` — per-rule working directory

```toml
[build-frontend]
type    = "task"
dir     = "frontend"
command = ["npm", "run", "build"]
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

---

## CLI reference

```
Usage: pbuild [OPTIONS] [TARGET]
       pbuild init
       pbuild status [TARGET]
       pbuild clean
       pbuild why <TARGET>

Options:
  -j <N>, --jobs <N>   Run at most N rules in parallel (default: logical CPUs)
  -n, --dry-run        Print commands without running them
  -k, --keep-going     Keep building independent rules after a failure
  -v, --verbose        Print skipped rules
  -l, --list           List all available targets and exit
  -h, --help           Print this help and exit
      --trust          Skip safety checks for dangerous commands
      --only           Build just the named target without running its dependencies
      --log <file>     Tee pbuild's output lines to a file (appends; no ANSI codes)

Special targets:
  init                 Write a starter pbuild.toml in the current directory
  status [TARGET]      Show which targets are dirty (would rebuild)
  clean                Delete all rule outputs and .pbuild.lock
  why <TARGET>         Explain why a target would rebuild
```

### `pbuild init`

Generates a commented starter `pbuild.toml` in the current directory. Errors if one already exists.

### `pbuild status`

Shows which targets are dirty (would rebuild) without running anything:

```
  build    dirty
  test     clean
  lint     clean
```

### `pbuild why <target>`

Explains exactly why a target would rebuild — lists each input file and whether it changed, any dep that was rebuilt, and any tracked env var that changed.

### `--only`

Builds a single target without running its dependencies first. Useful when you know deps are already up to date:

```sh
pbuild --only test
```

### `--log <file>`

Tees pbuild's own output lines to a file in addition to the terminal. The log file contains plain text with no ANSI color codes and appends on each run:

```sh
pbuild --log build.log
```

---

## How it works

1. `pbuild.toml` is parsed into a list of rules.
2. A dependency graph is built and topologically sorted.
3. Rules are executed in waves — all rules whose dependencies are satisfied run in parallel, bounded by `-j`.
4. After each wave, input and output files are hashed and flushed to `.pbuild.lock`.
5. On the next run, rules whose hashes haven't changed are skipped.

---

## License

MIT — see [LICENSE](LICENSE)
