# Docker Images for Memvid

This directory contains adjacent Docker tooling for the `memvid-core` crate. The crate contract and
supported profile matrix live in [README.md](../README.md); these images inherit that contract and
do not redefine it.

## Available Images

### Memvid Core (`core/`)

The Memvid Core Docker assets provide containerized development, test, and release-build
environments around the Rust crate. They are primarily for contributor workflows and CI parity.

**Quick Start:**
```bash
# Development environment
cd core
docker-compose up -d dev
docker-compose exec dev bash

# Run tests
docker-compose run --rm test

# Build release
docker-compose run --rm build
```

For detailed usage, see [core/README.md](core/README.md).

### Memvid CLI (`cli/`)

The Memvid CLI image provides an adjacent command-line workflow around the crate. It is useful when
you want a containerized CLI without installing Node.js or platform-specific binaries, but it is
not the definition of the crate support contract.

**Quick Start:**

```bash
# Pull the image
docker pull memvid/cli

# Create a memory
docker run --rm -v $(pwd):/data memvid/cli create my-memory.mv2

# Add documents
docker run --rm -v $(pwd):/data memvid/cli put my-memory.mv2 --input doc.pdf

# Search
docker run --rm -v $(pwd):/data memvid/cli find my-memory.mv2 --query "search"
```

For detailed usage instructions, examples, and Docker Compose configurations, see [cli/README.md](cli/README.md).

## Building Images

### Build CLI Image Locally

```bash
cd cli
docker build -t memvid/cli:test .
```

## Publishing

Docker images are automatically built and published to Docker Hub via GitHub Actions when tags are pushed. See `.github/workflows/docker-release.yml` for the CI/CD configuration.

**Image Registry:**
- Docker Hub: `memvid/cli`
- Tags: `latest` and version-specific tags

## Architecture Support

The CLI image supports multi-architecture builds:
- `linux/amd64`
- `linux/arm64`

## Security

The CLI image runs as a non-root user (`memvid`) for improved security. When mounting volumes, ensure your host directories have appropriate permissions.

## Links

- [Core Documentation](core/README.md)
- [CLI Documentation](cli/README.md)
- [CLI Testing Guide](cli/TESTING.md)
- [Main Project README](../README.md)
- [Memvid Documentation](https://docs.memvid.com)
