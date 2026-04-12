# Contributing to symgraph

Thank you for your interest in contributing to symgraph!

## Development Setup

1. Clone the repository:
   ```bash
   git clone https://github.com/grahambrooks/symgraph
   cd symgraph
   ```

2. Build the project:
   ```bash
   cargo build
   ```

3. Run tests:
   ```bash
   cargo test
   ```

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and address any warnings
- Add tests for new functionality

## Pull Request Process

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Make your changes
4. Run `cargo fmt` and `cargo clippy`
5. Commit your changes (`git commit -m 'Add amazing feature'`)
6. Push to the branch (`git push origin feature/amazing-feature`)
7. Open a Pull Request

## Adding Language Support

To add support for a new language:

1. Add the tree-sitter grammar dependency to `Cargo.toml`
2. Add the language enum variant in `src/types.rs`
3. Add language configuration in `src/extraction/languages.rs`
4. Update the `Language::from_extension` method
5. Add tests for the new language

## Reporting Issues

When reporting issues, please include:

- Your operating system and version
- Rust version (`rustc --version`)
- Steps to reproduce the issue
- Expected vs actual behavior
- Any relevant error messages

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
