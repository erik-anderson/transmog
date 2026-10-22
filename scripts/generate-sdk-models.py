"""Generate C/C++/managed ABI projections directly from canonical Rust declarations."""
import argparse
import json
import subprocess

from repo_tools import ROOT, command, development_environment, main_guard
from sdk_models import Generator, Models


def generate(check=False):
    env=development_environment()
    env.pop("CARGO_BUILD_TARGET", None)
    env.pop("RUSTC_BOOTSTRAP", None)
    env["CARGO_TARGET_DIR"]=str(ROOT / ".local/sdk/model-codegen-target")
    try:
        schema=command(["cargo","run","--quiet","--locked","--manifest-path",ROOT/"sdk/Cargo.toml",
                    "-p","transmog-sdk-model-codegen","--",
                    "transmog_app=crates/app/src/lib.rs", "transmog_core=crates/proxy-core/src/lib.rs",
                    "transmog_session=crates/proxy-session/src/lib.rs"], env=env,capture=True)
    except subprocess.CalledProcessError as error:
        raise ValueError(f"Rust model extraction failed:\n{error.stderr}") from error
    generator=Generator(Models(json.loads(schema.stdout)))
    rust=command(["rustfmt","--edition","2024"],env=env,input=generator.rust_code(),capture=True).stdout
    outputs={
        "sdk/include/transmog/models.h":generator.header(),
        "sdk/engine/src/models.rs":rust,
        "sdk/src/model_layout.inc":generator.layout_cpp(),
        "sdk/managed/Models.g.cs":generator.csharp(),
        "sdk/tests/models_generated.py":generator.python_fixture(),
        "sdk/tests/fake/model_layout.inc":generator.test_layout_cpp(),
    }
    for name,content in outputs.items():
        path=ROOT/name
        if check:
            if not path.exists() or path.read_text(encoding="utf-8")!=content:
                raise ValueError(f"{name} is stale; run python scripts/generate-sdk-models.py")
        else:
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_text(content,encoding="utf-8",newline="\n")
    print(f"{'Checked' if check else 'Generated'} {len(generator.m.selected)} canonical Rust model projections")


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check",action="store_true")
    generate(parser.parse_args().check)


if __name__=="__main__":
    main_guard(main)
