# Java Implementation (vibepod)

## Scope

- JDK version: JDK 25 (LTS, released 2025-09) baseline; newer JDK versions are acceptable. Earlier versions are out of scope. Use modern Java idioms (records, pattern matching for switch, sealed classes, `var` inference, text blocks, switch expressions, virtual threads, `Optional`, scoped values).
- Framework: none. Spring Boot opinions are NOT in this bundle — use a custom template for framework-specific workflows.
- Build tool: agnostic. Gradle and Maven both fine; the template does not mandate either.

## Rules (universal + Java-specific universal bugs)

- Write tests before implementation. Follow `skill: tdd-workflow`.
- Do not write empty `catch (Exception e) {}` or `catch (Throwable t) {}`. Either re-throw with context (`throw new IllegalStateException("context", e)`), handle explicitly, or document the reason for ignoring with a comment.
- Do not swallow `InterruptedException`. If catching is unavoidable, restore the interrupt flag before returning: `Thread.currentThread().interrupt();`.
- Compare objects with `.equals(...)`, not `==`. `==` on boxed primitives, `String`, or enum-looking values is a bug waiting to happen. Use `Objects.equals(a, b)` when either side may be `null`.
- If you override `equals`, override `hashCode` with a consistent implementation (or use a `record`, which derives both). Violating the contract silently breaks `HashMap` / `HashSet` lookups.
- Use try-with-resources for `Closeable` / `AutoCloseable`. Do not write manual `try/finally { stream.close(); }` blocks.
- Do not modify a collection while iterating over it. Use `Iterator.remove()`, stream `.filter(...)`, or iterate over a copy.
- No stray `System.out.println` / `e.printStackTrace()` for debugging in committed code. Use `java.util.logging`, `SLF4J`, or a project-specific logger with appropriate level.
- No hardcoded secrets.
- Before declaring work complete: project's build (`./gradlew build` or `mvn verify`), linter (if the project uses Checkstyle / ErrorProne / SpotBugs / PMD), and tests must all pass. vibepod enforces the discipline of running them all, not the choice.

## Agents (prefer in this order)

- `java-reviewer` — Java-specific review (null handling, collection misuse, concurrency primitives, resource management, modern Java idioms).
- `java-build-resolver` — when `./gradlew build` or `mvn verify` fails.
- `code-architect` — design / planning before implementation.
- `code-explorer` — tracing existing code paths in an unfamiliar module.
- `silent-failure-hunter` — before commit, sweep for swallowed exceptions (empty `catch`, `catch (Exception)` without context), misleading fallbacks.
- `code-reviewer` — generic fallback for cross-cutting concerns (API shape, naming, comments) when the java-specific reviewer has nothing to add.

## Skills (load as needed)

- `java-coding-standards` — idiomatic Java (records, sealed types, `Optional`, modern collection APIs, null discipline).
- `tdd-workflow` — strict RED → GREEN → REFACTOR discipline with git checkpoints.
