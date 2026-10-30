# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Exercise the real Windows status-file sharing race without a server or game."""
import ctypes
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest

from benchmark_startup import read


@unittest.skipUnless(os.name == "nt", "Windows file sharing")
class StatusReads(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "status.json"
        self.path.write_text('{"state":"ready"}')
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_ulong,
            ctypes.c_ulong, ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong,
            ctypes.c_void_p]
        self.kernel.CreateFileW.restype = ctypes.c_void_p
        self.kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        self.kernel.CloseHandle.restype = ctypes.c_int

    def lock(self):
        handle = self.kernel.CreateFileW(str(self.path), 0x80000000, 0, None, 3, 0, None)
        if handle == ctypes.c_void_p(-1).value:
            raise ctypes.WinError(ctypes.get_last_error())
        return handle

    def test_temporary_exclusive_handle_releases(self):
        handle = self.lock()
        release = threading.Timer(0.1, self.kernel.CloseHandle, args=(handle,))
        release.start()
        try:
            self.assertEqual(read(self.path), {"state": "ready"})
        finally:
            release.join()

    def test_persistent_exclusive_handle_fails(self):
        handle = self.lock()
        try:
            with self.assertRaises(PermissionError):
                read(self.path)
        finally:
            self.kernel.CloseHandle(handle)

    def test_malformed_content_is_not_hidden(self):
        self.path.write_text("not json")
        with self.assertRaises(json.JSONDecodeError):
            read(self.path)


if __name__ == "__main__":
    unittest.main()
