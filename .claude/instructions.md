# Project Instructions for Claude

## Git and Version Control

- **NEVER create git commits** - Only the user creates commits
- **NEVER run git add, git commit, or git push commands**
- The user owns all files in this project and manages version control themselves

## Commit Messages

When the user is ready to commit, they will ask for a commit message suggestion.
- Provide a concise, single-line commit message when possible
- Follow the format: `<action>: <brief description>`
- Keep it under 72 characters if possible
- Be descriptive but concise

## Project Overview

This is TranDB, a distributed in-memory key-value database written in Rust. See SPEC.md for full project specifications.

## Development Guidelines

**Testing Requirements:**
- Every code change MUST include corresponding unit tests
- Tests should cover new functionality, edge cases, and error conditions
- Code without tests is incomplete
- Unit tests MUST reside in their own dedicated files (e.g. `tests/unit_foo.rs`), NOT in inline `#[cfg(test)]` modules within source files
- After writing tests, ALWAYS run `cargo llvm-cov` to verify coverage. Use the env vars from the Justfile if needed (`LLVM_COV` / `LLVM_PROFDATA`)
- All new code must be covered. Uncovered lines must be either tested or explicitly justified (e.g. `run()` which blocks forever is inherently not unit-testable)

## Specification Workflow

Specifications are developed iteratively through collaboration:

**Initial Draft:**
- Start each new spec with a "Questions for Clarification" section (2-5 key questions)
- Present the initial specification structure

**Iteration Cycle:**
- User provides feedback either:
  - By answering questions directly in the document
  - By adding comments in `<<...>>` format within the spec
- On each iteration:
  - Integrate user answers and comments into the appropriate sections
  - Remove the `<<...>>` markers once addressed
  - Remove the "Questions for Clarification" section once all answers are integrated
- Continue iterating until the spec is finalized

**Key Principle:** Each revision should cleanly incorporate feedback so the spec remains readable without leftover questions or comment markers.