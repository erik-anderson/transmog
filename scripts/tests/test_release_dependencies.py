"""Keep isolated release consumers independent of another job's node_modules."""
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


def consumer_setup_missing(workflow):
    jobs = list(re.finditer(r"(?m)^  ([a-z][a-z_-]*):\s*$", workflow))
    sections = {job[1]: workflow[job.end():jobs[index + 1].start() if index + 1 < len(jobs) else len(workflow)]
                for index, job in enumerate(jobs)}
    consumers = {"validation": "Validation gate - Native", "package_compile": "Package gate - Build",
                 "package": "Package gate - Qualify"}
    missing = []
    for job, consumer in consumers.items():
        setup = sections[job].split("name: " + consumer)[0]
        if (not re.search(r"(?m)^\s+npm ci --ignore-scripts\s*$", setup)
                or "await import('./scripts/binary-reference-fixtures.mjs')" not in setup):
            missing.append(job)
    return missing


class ReleaseDependencyTests(unittest.TestCase):
    def test_package_metadata_receives_the_validated_policy_job_track(self):
        workflow = (ROOT / ".github/workflows/windows-release.yml").read_text()
        policy = workflow.split("  policy:", 1)[1].split("  engine:", 1)[0]
        package = workflow.split("  package:", 1)[1].split("  build:", 1)[0]
        self.assertIn("release-type: ${{ steps.versions.outputs.release-type }}", policy)
        version_step = policy.split("id: versions", 1)[1].split("      - name:", 1)[0]
        self.assertIn("python ./scripts/check-published-release-version.py", version_step)
        self.assertIn("RELEASE_TYPE: ${{ needs.policy.outputs.release-type }}", package)

    def test_each_isolated_native_or_package_consumer_installs_its_own_reference_encoders(self):
        workflow = (ROOT / ".github/workflows/windows-release.yml").read_text(encoding="utf-8")
        self.assertEqual(consumer_setup_missing(workflow), [])
        for job in ("validation", "package_compile", "package"):
            start = workflow.index("  " + job + ":\n")
            changed = workflow[:start] + workflow[start:].replace("          npm ci --ignore-scripts\n", "", 1)
            self.assertIn(job, consumer_setup_missing(changed))

    def test_reference_encoder_dependencies_are_locked_at_repository_root(self):
        script = (ROOT / "scripts/binary-reference-fixtures.mjs").read_text(encoding="utf-8")
        imports = set(re.findall(r'from "([^".][^"]*)"', script)) - {"node:http", "node:url", "node:zlib"}
        self.assertTrue(imports)
        package = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
        lock = json.loads((ROOT / "package-lock.json").read_text(encoding="utf-8"))
        for name in imports:
            self.assertIn(name, package["devDependencies"])
            self.assertEqual(lock["packages"]["node_modules/" + name]["version"], package["devDependencies"][name])


if __name__ == "__main__":
    unittest.main()
