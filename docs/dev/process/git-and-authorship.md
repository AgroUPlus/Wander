# Git Workflow, Authorship & Non-Appropriation

## 1. AI Tools and Contributor Status

- AI models and automated code agents are assistive utilities only. They are **not** authors or contributors.
- **Do not add** `Co-Authored-By` trailers or metadata referencing AI models to git commit messages.
- Any use of AI tools must strictly adhere to [`CLA.md`](../../CLA.md) Section 8. No AI vendor or automated system acquires ownership, copyright, or licensing claims over project code.

## 2. Commit Discipline

- Keep commits atomic and clearly motivated.
- Ensure all quality gates (`cargo test`, `cargo clippy`, `cargo fmt`) pass before committing.
