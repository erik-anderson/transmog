"""Reproduce or verify the checksum-pinned transport dependency patches."""
from transport_dependencies import main
from repo_tools import main_guard

if __name__ == "__main__":
    main_guard(main)
