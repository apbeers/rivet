import unittest
from check_dependencies import check


class PolicyTests(unittest.TestCase):
    def metadata(self, owner, dep, source="registry+example", kind=None):
        return {"workspace_members": ["local"], "packages": [{
            "id": "local", "name": owner, "dependencies": [
                {"name": dep, "source": source, "kind": kind}
            ]
        }]}

    def test_adapter_exceptions(self):
        for dep in ["github-copilot-sdk", "tokio"]:
            self.assertEqual(check(self.metadata("rivet-copilot", dep)), [])

    def test_other_crates_cannot_add_external_dependencies(self):
        for kind in [None, "dev", "build"]:
            self.assertTrue(check(self.metadata("rivet", "tokio", kind=kind)))
            self.assertTrue(check(self.metadata("rivet-copilot", "serde", kind=kind)))

    def test_native_bindings_are_macos_target_only(self):
        m = self.metadata("rivet-macos", "objc2")
        self.assertTrue(check(m))
        m["packages"][0]["dependencies"][0]["target"] = 'cfg(target_os = "macos")'
        self.assertEqual(check(m), [])
        self.assertTrue(check(self.metadata("rivet", "objc2")))

    def test_unregistered_path_dependency_is_rejected(self):
        self.assertTrue(check(self.metadata("rivet", "vendored-library", source=None)))


if __name__ == "__main__":
    unittest.main()
