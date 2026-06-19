#!/usr/bin/env python3
"""First-party markitdown entry point for the jailed child (M5b). Bytes on
stdin -> Markdown on stdout. argv[1] = extension hint (no path). Strips the
converter registry to a minimal OFFLINE set AND asserts the strip worked, so a
markitdown bump that breaks it fails LOUDLY (exit 9) instead of silently.
requests.Session() is created unconditionally inside MarkItDown; that latent
network capability is acceptable ONLY because the jail (later tasks) denies
network. The strip is defense-in-depth + attack-surface minimization."""
import sys, resource
for r, l in ((resource.RLIMIT_AS, 1 << 30), (resource.RLIMIT_CPU, 20), (resource.RLIMIT_FSIZE, 64 << 20)):
    try:
        resource.setrlimit(r, (l, l))
    except (ValueError, OSError):
        pass
sys.stdout.reconfigure(encoding="utf-8")
from markitdown import MarkItDown, StreamInfo
ALLOW = {"PdfConverter", "DocxConverter", "PptxConverter", "XlsxConverter", "XlsConverter", "OutlookMsgConverter", "PlainTextConverter"}
BANNED = {"RssConverter", "WikipediaConverter", "YouTubeConverter", "BingSerpConverter"}
md = MarkItDown(enable_plugins=False, enable_builtins=True)
md._converters = [c for c in md._converters if type(getattr(c, "converter", c)).__name__ in ALLOW]
names = {type(getattr(c, "converter", c)).__name__ for c in md._converters}
if not names or (names & BANNED):
    sys.stderr.write(f"registry strip failed: kept={sorted(names)}\n")
    sys.exit(9)
ext = sys.argv[1] if len(sys.argv) > 1 else None
si = StreamInfo(extension=("." + ext)) if ext else None
sys.stdout.write(md.convert_stream(sys.stdin.buffer, stream_info=si).text_content)
