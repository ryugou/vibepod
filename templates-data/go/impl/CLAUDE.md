# Go Implementation (vibepod)

## Rules (universal + Go-specific)

- Write tests before implementation. Follow `skill: tdd-workflow`.
- Never discard errors silently. Every `err != nil` path must be handled — returned with context (`fmt.Errorf("...: %w", err)`), logged with actionable information, or explicitly documented with a comment if ignoring is deliberate.
- Do not use `_ = someFunc()` to discard error-returning calls. Either handle the error or add a comment stating why discarding is safe.
- Do not `panic` on recoverable errors. `panic` is for impossible states; recoverable paths return an error.
- No stray `fmt.Println` / `log.Println` for debugging in committed code. Use `log/slog` (or a project-specific logger) with appropriate level.
- No hardcoded secrets.
- Before declaring work complete: `gofmt -l .` must print nothing, `go vet ./...` must be clean, `go test ./...` must all pass. Project-specific linters (`golangci-lint`, `staticcheck`) also apply if the project uses them.

## Agents (prefer in this order)

- `go-reviewer` — Go-specific review (error handling, goroutine leaks, context propagation, idioms).
- `go-build-resolver` — when `go build` or `go test` fails.
- `code-architect` — design / planning before implementation.
- `code-explorer` — tracing existing code paths in an unfamiliar module.
- `silent-failure-hunter` — before commit, sweep for swallowed errors and bad fallbacks.
- `code-reviewer` — generic fallback for cross-cutting concerns (API shape, naming, comments) when the go-specific reviewer has nothing to add.

## Skills (load as needed)

- `golang-patterns` — idiomatic error handling, interfaces, context propagation, concurrency.
- `golang-testing` — table-driven tests, `testify`, race detector, coverage.
- `tdd-workflow` — strict RED → GREEN → REFACTOR discipline with git checkpoints.
