"""Distribution guards; fixture binaries contain headers only and are never executed."""
import pathlib
import struct
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import publish_preview as release

REPO = 'apbeers/rivet'
SHA = 'a' * 40
RUN = '12345'


class PreviewTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.artifacts = self.root / 'artifacts'
        self.output = self.root / 'downloads'
        self.windows = self.artifacts / 'windows/Rivet-Windows-x64-development.zip'
        self.mac = self.artifacts / 'macos/Rivet-macOS-development.zip'
        dos = bytearray(64)
        dos[:2] = b'MZ'
        struct.pack_into('<I', dos, 60, 64)
        self.pe = dos + b'PE\0\0\x64\x86'
        self.package(self.windows, release.WINDOWS_FILES, self.pe)
        self.package(self.mac, release.MAC_FILES, struct.pack('<II', 0xfeedfacf, 0x0100000c))

    def package(self, path, names, header):
        path.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(path, 'w') as archive:
            for index, name in enumerate(names):
                archive.writestr(name, header if index == 0 else b'distribution test fixture')

    def prepare(self):
        release.prepare(self.artifacts, self.output, REPO, SHA, RUN)

    def test_wrong_architecture_or_missing_runtime_is_not_published(self):
        self.package(self.mac, release.MAC_FILES, struct.pack('<II', 0xfeedfacf, 0x01000007))
        with self.assertRaisesRegex(ValueError, 'Apple Silicon'):
            self.prepare()
        self.assertFalse(self.output.exists())
        self.package(self.mac, release.MAC_FILES, struct.pack('<II', 0xfeedfacf, 0x0100000c))
        self.package(self.windows, tuple(n for n in release.WINDOWS_FILES if n != 'WebView2Loader.dll'), self.pe)
        with self.assertRaisesRegex(ValueError, 'incomplete package'):
            self.prepare()
        self.assertFalse(self.output.exists())

    @patch.dict('os.environ', {'GITHUB_REF': 'refs/heads/main'})
    def test_changed_download_is_rejected_before_any_github_request(self):
        self.prepare()
        release.verify_output(self.output, REPO, SHA, RUN)
        with (self.output / 'Rivet-Windows-x64.zip').open('ab') as changed:
            changed.write(b'changed after preparation')
        with patch.object(release, 'api') as github:
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                release.publish(self.output, REPO, SHA, RUN)
            github.assert_not_called()

    def test_source_commit_and_run_must_match_downloads(self):
        self.prepare()
        with self.assertRaisesRegex(ValueError, 'provenance'):
            release.verify_output(self.output, REPO, 'b' * 40, RUN)
        with self.assertRaisesRegex(ValueError, 'provenance'):
            release.verify_output(self.output, REPO, SHA, '67890')

    @patch.dict('os.environ', {'GITHUB_REF': 'refs/heads/main'})
    def test_unmanaged_release_is_never_overwritten(self):
        self.prepare()
        responses = [{'object': {'sha': SHA}}, {'body': 'A manually maintained release', 'prerelease': True}]
        with patch.object(release, 'api', side_effect=responses), patch.object(release.subprocess, 'run') as upload:
            with self.assertRaisesRegex(ValueError, 'not owned'):
                release.publish(self.output, REPO, SHA, RUN)
            upload.assert_not_called()

    @patch.dict('os.environ', {'GITHUB_REF': 'refs/heads/main'})
    def test_superseded_build_cannot_replace_current_downloads(self):
        self.prepare()
        with patch.object(release, 'api', return_value={'object': {'sha': 'b' * 40}}) as github:
            with patch.object(release.subprocess, 'run') as upload:
                release.publish(self.output, REPO, SHA, RUN)
                upload.assert_not_called()
            self.assertEqual(github.call_count, 1)

    @patch.dict('os.environ', {'GITHUB_REF': 'refs/heads/feature'})
    def test_feature_branch_cannot_publish(self):
        with patch.object(release, 'api') as github:
            with self.assertRaisesRegex(ValueError, 'main-branch'):
                release.publish(self.output, REPO, SHA, RUN)
            github.assert_not_called()


if __name__ == '__main__':
    unittest.main()
