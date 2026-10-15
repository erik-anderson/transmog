"""Wire vectors and failure checks for the local Chromium comparison."""
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import browser_fingerprint as fp


def vector(data, width=2):
    return len(data).to_bytes(width, "big") + data


def hello(extensions, ciphers=b"\x0a\x0a\x13\x01\x13\x02"):
    return (b"\x03\x03" + bytes(32) + b"\0" + vector(ciphers) + b"\x01\0"
            + vector(b"".join(kind.to_bytes(2, "big") + vector(value) for kind, value in extensions)))


def frame(kind, flags, stream, payload=b""):
    return len(payload).to_bytes(3, "big") + bytes([kind, flags]) + stream.to_bytes(4, "big") + payload


class TlsTests(unittest.TestCase):
    def test_published_ja4_example(self):
        # https://github.com/FoxIO-LLC/ja4/blob/main/technical_details/JA4.md
        ciphers = fp.codes(bytes.fromhex("130113021303c02bc02fc02cc030cca9cca8c013c014009c009d002f0035"))
        extensions = fp.codes(bytes.fromhex("001b00000033001044690017002d000d000500230012002bff01000b000a0015"))
        signatures = fp.codes(bytes.fromhex("04030804040105030805050108060601"))
        self.assertEqual(fp.ja4(ciphers, extensions, signatures, [0x0304], 0x0303, [b"h2"], "t"),
                         "t13d1516h2_8daaf6152771_e5627efa2ab1")

    def test_ja4_hash_inputs_and_signature_order(self):
        # Independent expected strings, including exclusion of GREASE/SNI/ALPN.
        ciphers = [0x0a0a, 0x1302, 0x1301]
        extensions = [0x1a1a, 43, 16, 0, 13, 10]
        h = lambda s: hashlib.sha256(s.encode()).hexdigest()[:12]
        expected = "t13d0205h2_" + h("1301,1302") + "_" + h("000a,000d,002b_0804,0403")
        actual = fp.ja4(ciphers, extensions, [0x0804, 0x0403], [0x2a2a, 0x0304], 0x0303, [b"h2"], "t")
        self.assertEqual(actual, expected)
        self.assertEqual(actual, fp.ja4(ciphers[::-1], extensions[::-1], [0x0804, 0x0403], [0x0304], 0x0303, [b"h2"], "t"))
        self.assertNotEqual(actual, fp.ja4(ciphers, extensions, [0x0403, 0x0804], [0x0304], 0x0303, [b"h2"], "t"))

    def test_empty_ja4_and_non_ascii_alpn(self):
        self.assertEqual(fp.ja4([], [], [], [], 0x0303, [], "t"), "t12i000000_000000000000_000000000000")
        self.assertTrue(fp.ja4([], [], [], [0x0304], 0x0303, [b"\x01\xff"], "q").startswith("q13i00000f_"))

    def test_hello_preserves_orders_and_normalizes_only_grease(self):
        extensions = [(0, vector(b"\0" + vector(b"localhost"))), (43, vector(b"\x1a\x1a\x03\x04", 1)),
                      (10, vector(b"\x2a\x2a\x00\x1d")), (13, vector(b"\x08\x04\x04\x03")),
                      (16, vector(vector(b"h2", 1))), (51, vector(b"\x00\x1d" + vector(bytes(32)))), (0x3a3a, b"\0")]
        parsed = fp.client_hello(hello(extensions))
        self.assertEqual(parsed["normalized"]["ciphers"], ["GREASE", "1301", "1302"])
        self.assertEqual(parsed["normalized"]["groups"], ["GREASE", "001d"])
        self.assertEqual(parsed["normalized"]["key_shares"], [{"group":"001d", "length":32}])
        self.assertEqual(parsed["extension_order"][:2], ["0000", "002b"])
        self.assertEqual(parsed["normalized"], fp.client_hello(hello(extensions[::-1]))["normalized"])

    def test_truncated_vectors_and_duplicate_extensions_fail(self):
        valid = hello([(43, b"\x02\x03\x04")])
        for n in range(len(valid)):
            with self.subTest(length=n), self.assertRaises(ValueError):
                fp.client_hello(valid[:n])
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            fp.client_hello(hello([(43, b"\x02\x03\x04")] * 2))

    def test_quic_varints_duplicate_and_truncation(self):
        data = b"\x04\x02\x40\x80\x0f\x04abcd\x1b\x01x"
        parsed = fp.quic_parameters(data)
        self.assertEqual(parsed["parameters"], {"4":128, "15":{"length":4}})
        self.assertEqual(parsed["grease_count"], 1)
        self.assertEqual(len(parsed["wire_order"]), 3)
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            fp.quic_parameters(b"\x04\x01\x01" * 2)
        for invalid in (b"\xc0", b"\x04\x02\x40", b"\x04\x02\x01\x02"):
            with self.assertRaises(ValueError): fp.quic_parameters(invalid)


class HttpTests(unittest.TestCase):
    def test_h2_continuation_padding_priority_and_window(self):
        wire = (fp.PREFACE + frame(4, 0, 0, b"\x00\x01\0\x01\0\0") + frame(8, 0, 0, b"\0\0\x10\0")
                + frame(1, 0x28, 1, b"\x02\x80\0\0\0\xffabc\0\0") + frame(9, 4, 1, b"def"))
        frames, block = fp.h2_frames(wire)
        self.assertEqual(block, b"abcdef")
        self.assertEqual(frames[0]["settings"], [[1, 65536]])
        self.assertEqual(frames[1]["window_increment"], 4096)
        self.assertEqual(frames[2]["priority"], {"dependency":0, "exclusive":True, "weight":256})

    def test_h2_missing_or_interleaved_headers_fail(self):
        invalid = [frame(1, 4, 1, b"a"), frame(4, 1, 0), frame(4, 0, 1),
                   frame(4, 0, 0) + frame(1, 0, 1, b"a"),
                   frame(4, 0, 0) + frame(1, 0, 1, b"a") + frame(8, 0, 0, bytes(4)),
                   frame(4, 0, 0) + frame(1, 0, 1, b"a") + frame(9, 4, 3, b"b")]
        for wire in invalid:
            with self.subTest(wire=wire), self.assertRaises(ValueError): fp.h2_frames(fp.PREFACE + wire)

    def test_h1_order_case_duplicates_and_cookie_inputs(self):
        wire = b"GET /probe HTTP/1.1\r\nHost: localhost:1234\r\nX-One: a\r\nx-one: b\r\nCookie: z=2; a=1\r\nAccept-Language: en-US\r\nReferer: https://example.test/\r\n\r\n"
        measured = fp.application({"protocol":"h1", "local":"127.0.0.1:1234", "application_bytes":wire.hex()})["http"]
        self.assertEqual(measured["normalized"]["header_order"], ["host", "x-one", "x-one", "cookie", "accept-language", "referer"])
        self.assertEqual(measured["wire_headers"][1], ("X-One", "a"))
        self.assertEqual(measured["normalized"]["cookie_names"], ["a", "z"])
        self.assertTrue(measured["normalized"]["referer_present"])
        self.assertEqual(measured["normalized"]["headers"][0], ("host", "localhost:PORT"))
        self.assertEqual(measured["normalized"]["header_casing"][1], "X-One")
        wrong = fp.application({"protocol":"h1", "local":"127.0.0.1:4567", "application_bytes":wire.hex()})["http"]
        self.assertEqual(wrong["normalized"]["headers"][0], ("host", "localhost:1234"))

    def test_h3_requires_peer_settings(self):
        with self.assertRaisesRegex(ValueError, "SETTINGS"):
            fp.application({"protocol":"h3", "local":"127.0.0.1:443", "headers":[[":method","GET"], [":path","/"]], "settings":None})


class TcpAndReportTests(unittest.TestCase):
    def test_h3_reserved_settings_keep_position_and_do_not_mask_real_values(self):
        def measurement(settings):
            return {"tls":{"normalized":{},"ja4":"q"}, "client_hello_count":1,
                    "negotiated":{}, "http":{"normalized":{}}, "tcp":{"status":"not-applicable"},
                    "h3":{"settings":settings}}
        first = measurement([[1,65536], [33,42]])
        second = measurement([[1,65536], [64,123]])
        self.assertEqual(fp.comparison_fields(first), fp.comparison_fields(second))
        self.assertEqual(first["h3"]["settings"][1], [33,42])  # Raw evidence survives.
        self.assertTrue(fp.differences(fp.comparison_fields(first), fp.comparison_fields(measurement([[33,42], [1,65536]]))))
        self.assertTrue(fp.differences(fp.comparison_fields(first), fp.comparison_fields(measurement([[1,0], [33,42]]))))

    def packet(self):
        ip, tcp = bytearray(20), bytearray(20)
        ip[0], ip[8], ip[9], ip[12:16], ip[16:20] = 0x45, 64, 6, bytes([127,0,0,1]), bytes([127,0,0,1])
        tcp[:4], tcp[12], tcp[13], tcp[14:16] = b"\x80\x00\x01\xbb", 0x80, 2, b"\xff\xff"
        return bytes(12) + b"\x08\x00" + ip + tcp + b"\x02\x04\x05\xb4\x01\x03\x03\x07\x01\x01\0\0"

    def test_actual_tcp_syn_inputs_and_udp_not_applicable(self):
        actual = fp.tcp_syn(self.packet())
        self.assertEqual(actual["peer"], "127.0.0.1:32768")
        self.assertEqual(actual["normalized"], {"window":65535,"mss":1460,"window_scale":7,"ip_ttl":64,"option_order":[2,1,3,1,1,0]})
        self.assertIsNone(fp.tcp_syn(bytes(54)))
        capture = object.__new__(fp.SynCapture)
        capture.records = []
        self.assertEqual(capture.find({"protocol":"h3"})["status"], "not-applicable")
        with self.assertRaisesRegex(ValueError, "SYN"):
            capture.find({"protocol":"h2", "peer":"p", "local":"l"})

    def test_report_escapes_wire_values_and_preserves_evidence(self):
        result = {"environment":{"browser":"155"}, "within_client_variation":{}, "comparisons":[{"protocol":"h1", "sample":0,
                  "direct":{"tls":{"ja4":"test"}}, "proxied":{"tls":{"ja4":"test"}},
                  "differences":[{"field":"http.headers", "direct":"<script>bad</script>", "proxied":None}]}]}
        with tempfile.TemporaryDirectory() as temporary:
            fp.write_report(temporary, result)
            report = (Path(temporary) / "report.html").read_text()
            self.assertNotIn("<script>", report)
            self.assertIn("&lt;script&gt;", report)
            self.assertEqual(json.loads((Path(temporary) / "results.json").read_text()), result)


if __name__ == "__main__":
    unittest.main()
