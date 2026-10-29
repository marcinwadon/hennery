"""Copy only bounded pin data from an untrusted artifact into a trusted checkout."""
from pathlib import Path
import stat
import sys

FILES = (
    'adapters/pins.toml',
    'adapters/claude/package.json',
    'adapters/claude/package-lock.json',
    'adapters/codex/package.json',
    'adapters/codex/package-lock.json',
    'crates/hennery-host/manifest.json',
)
MAX_FILE_BYTES = 16 * 1024 * 1024


def regular_file(root, relative):
    path = root
    for part in Path(relative).parts:
        path = path / part
        if path.is_symlink():
            raise ValueError(f'symlink in pin data path: {relative}')
    if not path.resolve(strict=True).is_relative_to(root) or not stat.S_ISREG(path.stat().st_mode):
        raise ValueError(f'expected regular pin data: {relative}')
    return path


def copy_update(source, destination):
    if source.is_symlink() or destination.is_symlink():
        raise ValueError('pin data roots must not be symlinks')
    source, destination = source.resolve(strict=True), destination.resolve(strict=True)
    pending = []
    for relative in FILES:
        src, dst = regular_file(source, relative), regular_file(destination, relative)
        if src.stat().st_size > MAX_FILE_BYTES:
            raise ValueError(f'pin data too large: {relative}')
        with src.open('rb') as stream:
            data = stream.read(MAX_FILE_BYTES + 1)
        if len(data) > MAX_FILE_BYTES:
            raise ValueError(f'pin data too large: {relative}')
        pending.append((dst, data))
    # Validate and read every input before changing any tracked file. Extra
    # artifact paths, especially .git and executable files, are never copied.
    for dst, data in pending:
        dst.write_bytes(data)


if __name__ == '__main__':
    copy_update(Path(sys.argv[1]), Path(sys.argv[2]))
