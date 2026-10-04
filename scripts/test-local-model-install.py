"""Offline integrity checks: a partial artifact never becomes an accepted model."""
import hashlib
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("installer", Path(__file__).with_name("install-local-model.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class IntegrityTests(unittest.TestCase):
  def test_invalid_download_cannot_become_a_model(self):
    with tempfile.TemporaryDirectory() as directory:
      base = Path(directory)
      def downloaded(*args, **kwargs):
        (base / "model.gguf.part").write_bytes(b"incomplete")
      with patch.object(installer, "DIRECTORY", base), patch.object(installer.subprocess, "run", downloaded):
        with self.assertRaises(RuntimeError):
          installer.download("model.gguf", "https://example.invalid/model", "0" * 64)
      self.assertFalse((base / "model.gguf").exists())

  def test_verified_file_is_reusable_but_tampered_file_is_rejected(self):
    payload = b"synthetic artifact"
    with tempfile.TemporaryDirectory() as directory:
      base = Path(directory)
      def downloaded(*args, **kwargs):
        (base / "model.gguf.part").write_bytes(payload)
      digest = hashlib.sha256(payload).hexdigest()
      with patch.object(installer, "DIRECTORY", base), patch.object(installer.subprocess, "run", downloaded):
        installer.download("model.gguf", "https://example.invalid/model", digest)
      self.assertEqual((base / "model.gguf").read_bytes(), payload)
      self.assertFalse((base / "model.gguf.part").exists())
      with patch.object(installer, "DIRECTORY", base), patch.object(installer.subprocess, "run") as network:
        installer.download("model.gguf", "https://example.invalid/model", digest)
        (base / "model.gguf").write_bytes(b"changed")
        with self.assertRaises(RuntimeError):
          installer.download("model.gguf", "https://example.invalid/model", digest)
        network.assert_not_called()


if __name__ == "__main__":
  unittest.main()
