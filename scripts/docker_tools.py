"""Owned Docker validation containers and checked repository mounts."""
from repo_tools import ROOT, command, executable


def build_image(image):
    docker = executable("docker")
    command([docker, "build", "--file", ROOT / "fuzz/Dockerfile", "--tag", image, ROOT / "fuzz"])
    return docker


def container_arguments(docker, image, *, writable=False, mounts=(), environment=()):
    # --mount's CSV grammar cannot faithfully represent an arbitrary comma path.
    if "," in str(ROOT):
        raise RuntimeError("Docker repository mounts require a path without commas.")
    argv = [docker, "run", "--rm"]
    for value in environment:
        argv += ["--env", value]
    argv += ["--mount", f"type=bind,source={ROOT},target=/work" + ("" if writable else ",readonly")]
    for mount in mounts:
        argv += ["--mount", mount]
    return [*argv, "--workdir", "/work", image]
