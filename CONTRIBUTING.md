# Contributing to ProximaDB

Thank you for your interest in contributing to ProximaDB! We welcome contributions from the community.

## How to Contribute

### Reporting Issues
- Use the [GitHub Issues](https://github.com/vjsingh1984/proximadb/issues) tracker
- Check if the issue already exists
- Provide detailed reproduction steps

### Code Contributions

1. **Fork the Repository**
   ```bash
   git clone https://github.com/vjsingh1984/proximadb.git
   cd proximadb
   ```

2. **Create a Feature Branch**
   ```bash
   git checkout -b feature/your-feature-name
   ```

3. **Make Your Changes**
   - Follow the existing code style
   - Add tests for new features
   - Update documentation

4. **Run Tests**
   ```bash
   cargo test
   cargo clippy
   cargo fmt --check
   ```

5. **Submit a Pull Request**
   - Describe your changes clearly
   - Reference any related issues
   - Ensure CI passes

## Development Setup

### Prerequisites
- Rust 1.70+ 
- Docker (optional)
- Python 3.8+ (for SDK development)

### Building
```bash
cargo build --release
```

### Testing
```bash
cargo test --all
```

## Code Style
- Use `cargo fmt` for formatting
- Run `cargo clippy` for linting
- Follow Rust naming conventions

## Documentation
- Update docs for new features
- Add inline code comments
- Include examples

## Questions?
- Open a [GitHub Discussion](https://github.com/vjsingh1984/proximadb/discussions)
- Join our [Discord](https://discord.gg/proximadb)
- Email: singhvjd@gmail.com

## License
By contributing, you agree that your contributions will be licensed under the Apache 2.0 License.