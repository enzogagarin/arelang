# Compiler Architecture Seed

Arelang starts as an interpreter-first compiler toolchain. The first mission is a working HTTP server, not optimized native codegen.

## Bootstrap Language

Rust.

Reasons:

- strong enum and pattern matching support for AST and IR
- safe compiler internals
- good test tooling
- good fit for diagnostics and future region analysis

## Initial Crates

```text
crates/
  are-cli/           CLI entry point; owns commands like check, run, fmt
  are-diagnostics/   human and JSON diagnostic data model
  are-lexer/         source text to token stream
```

Next crates:

```text
crates/
  are-ast/
  are-parser/
  are-resolver/
  are-typecheck/
  are-runtime/
  are-http-runtime/
```

## First Pass Pipeline

```text
source.are
  -> lexer
  -> parser
  -> resolver
  -> type checker
  -> service registry
  -> interpreter/runtime
  -> HTTP server
```

## CLI Contract

```sh
are check examples/users_api
are check --json examples/users_api
are run examples/users_api
```

`are check --json` is the AI-agent contract. Its schema must remain stable once parser diagnostics begin.

## Diagnostic Shape

Diagnostics should include:

- code
- severity
- file
- range
- problem
- reason
- fixes

Security-impacting fixes should not be automatically applied.

