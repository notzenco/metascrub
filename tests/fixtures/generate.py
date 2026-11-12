#!/usr/bin/env python3
"""Regenerate the metadata-laden test fixtures in this directory.

The fixtures are committed, so tests do not need this script. It exists so
anyone can see exactly what metadata each fixture carries and rebuild them.

Requirements: Python 3 (stdlib only), macOS `sips` (JPEG/HEIC encoding) and
`cwebp` (WebP encoding). Run from anywhere:

    python3 tests/fixtures/generate.py

Every planted value contains one of the markers listed in MARKERS so the tests
can assert that cleaned output no longer contains them.
"""

import io
import os
import shutil
import struct
import subprocess
import tempfile
import zipfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))

# Strings planted in metadata. tests/common/mod.rs mirrors this list.
MAKE = "MetascrubCam"
MODEL = "MSX-1000"
SERIAL = "SN-0042-SECRET"
ARTIST = "Jane Example"
SOFTWARE = "MSFirmware 9.9"
DATE_EXIF = "2019:07:14 10:30:00"
DATE_ISO = "2019-07-14T10:30:00Z"
COMMENT = "Secret comment"
TRAILER = b"TRAILERSECRET"
COMPANY = "ExampleCorp Pty Ltd"
MARKERS = [MAKE, MODEL, SERIAL, ARTIST, SOFTWARE, COMMENT, COMPANY]

# Eiffel Tower: 48 deg 51' 29.88" N, 2 deg 17' 40.2" E
GPS_LAT = ((48, 1), (51, 1), (2988, 100))
GPS_LON = ((2, 1), (17, 1), (402, 10))

ASCII, SHORT, LONG, RATIONAL = 2, 3, 4, 5


def tiff(byte_order, ifd0, exif_ifd, gps_ifd):
    """Build a TIFF/EXIF blob with IFD0 plus Exif and GPS sub-IFDs."""
    e = ">" if byte_order == "MM" else "<"

    def encode(typ, value):
        if typ == ASCII:
            data = value.encode() + b"\0"
            return len(data), data
        if typ == SHORT:
            return len(value), b"".join(struct.pack(e + "H", v) for v in value)
        if typ == LONG:
            return len(value), b"".join(struct.pack(e + "I", v) for v in value)
        if typ == RATIONAL:
            return len(value), b"".join(struct.pack(e + "II", n, d) for n, d in value)
        raise ValueError(typ)

    ifds = [list(ifd0), list(exif_ifd), list(gps_ifd)]
    # Pointer tags are patched once offsets are known.
    ifds[0].append((0x8769, LONG, [0]))
    ifds[0].append((0x8825, LONG, [0]))
    for ifd in ifds:
        ifd.sort(key=lambda entry: entry[0])

    sizes = [2 + 12 * len(ifd) + 4 for ifd in ifds]
    offsets = [8]
    for size in sizes[:-1]:
        offsets.append(offsets[-1] + size)
    data_offset = offsets[-1] + sizes[-1]

    out = bytearray(byte_order.encode() + struct.pack(e + "HI", 42, 8))
    data = bytearray()
    for index, ifd in enumerate(ifds):
        out += struct.pack(e + "H", len(ifd))
        for tag, typ, value in ifd:
            if tag == 0x8769:
                value = [offsets[1]]
            elif tag == 0x8825:
                value = [offsets[2]]
            count, raw = encode(typ, value)
            if len(raw) <= 4:
                field = raw.ljust(4, b"\0")
            else:
                field = struct.pack(e + "I", data_offset + len(data))
                data += raw
                if len(data) % 2:
                    data += b"\0"
            out += struct.pack(e + "HHI", tag, typ, count) + field
        out += struct.pack(e + "I", 0)
    return bytes(out + data)


def exif_blob(byte_order="MM"):
    ifd0 = [
        (0x010F, ASCII, MAKE),
        (0x0110, ASCII, MODEL),
        (0x0112, SHORT, [6]),  # Orientation: rotate 90 CW
        (0x0131, ASCII, SOFTWARE),
        (0x0132, ASCII, DATE_EXIF),
        (0x013B, ASCII, ARTIST),
    ]
    exif_ifd = [
        (0x9003, ASCII, DATE_EXIF),  # DateTimeOriginal
        (0xA431, ASCII, SERIAL),  # BodySerialNumber
        (0xA434, ASCII, MODEL + " Lens"),  # LensModel
    ]
    gps_ifd = [
        (0x0001, ASCII, "N"),
        (0x0002, RATIONAL, GPS_LAT),
        (0x0003, ASCII, "E"),
        (0x0004, RATIONAL, GPS_LON),
    ]
    return tiff(byte_order, ifd0, exif_ifd, gps_ifd)


def xmp_packet():
    return (
        '<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>'
        '<x:xmpmeta xmlns:x="adobe:ns:meta/">'
        '<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
        '<rdf:Description rdf:about=""'
        ' xmlns:xmp="http://ns.adobe.com/xap/1.0/"'
        ' xmlns:dc="http://purl.org/dc/elements/1.1/"'
        f' xmp:CreatorTool="{SOFTWARE}" xmp:CreateDate="{DATE_ISO}">'
        f"<dc:creator><rdf:Seq><rdf:li>{ARTIST}</rdf:li></rdf:Seq></dc:creator>"
        "</rdf:Description></rdf:RDF></x:xmpmeta>"
        '<?xpacket end="w"?>'
    ).encode()


# A stand-in ICC payload. The cleaner must keep it byte-for-byte.
ICC = b"ICC_PROFILE\0\x01\x01" + b"metascrub-test-icc-profile"


def png_chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def base_png(w=16, h=16):
    rows = b"".join(
        b"\0" + b"".join(bytes([(x * 16) % 256, (y * 16) % 256, 128]) for x in range(w))
        for y in range(h)
    )
    return (
        b"\x89PNG\r\n\x1a\n"
        + png_chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + png_chunk(b"IDAT", zlib.compress(rows))
        + png_chunk(b"IEND", b"")
    )


def jpeg_segments(data):
    """Yield (marker, payload) up to and including SOS, then the rest."""
    assert data[:2] == b"\xff\xd8"
    i = 2
    while True:
        marker = data[i + 1]
        length = struct.unpack(">H", data[i + 2 : i + 4])[0]
        if marker == 0xDA:
            yield marker, data[i:]
            return
        yield marker, data[i : i + 2 + length]
        i += 2 + length


def segment(marker, payload):
    return bytes([0xFF, marker]) + struct.pack(">H", len(payload) + 2) + payload


def photoshop_irb():
    """APP13 payload holding one IPTC record: 2:80 By-line."""
    iptc = b"\x1c\x02\x50" + struct.pack(">H", len(ARTIST)) + ARTIST.encode()
    resource = b"8BIM" + struct.pack(">H", 0x0404) + b"\0\0" + struct.pack(">I", len(iptc)) + iptc
    if len(iptc) % 2:
        resource += b"\0"
    return b"Photoshop 3.0\0" + resource


def make_jpeg(tmp):
    png_path = os.path.join(tmp, "base.png")
    jpg_path = os.path.join(tmp, "base.jpg")
    with open(png_path, "wb") as f:
        f.write(base_png())
    subprocess.run(
        ["sips", "-s", "format", "jpeg", png_path, "--out", jpg_path],
        check=True,
        capture_output=True,
    )
    with open(jpg_path, "rb") as f:
        encoded = f.read()
    # Drop everything sips added (JFIF, EXIF, Photoshop) and plant our own.
    kept = [seg for marker, seg in jpeg_segments(encoded) if not (0xE0 <= marker <= 0xEF)]
    out = b"\xff\xd8"
    out += segment(0xE0, b"JFIF\0\x01\x01\x00\x00\x48\x00\x48\x00\x00")
    out += segment(0xE1, b"Exif\0\0" + exif_blob("MM"))
    out += segment(0xE1, b"http://ns.adobe.com/xap/1.0/\0" + xmp_packet())
    out += segment(0xE2, ICC)
    out += segment(0xED, photoshop_irb())
    out += segment(0xFE, COMMENT.encode())
    out += b"".join(kept)
    return out + TRAILER


def make_png():
    png = base_png()
    ihdr_end = 8 + 25
    idat_start = ihdr_end
    xmp = b"XML:com.adobe.xmp\0\0\0\0\0" + xmp_packet()
    extra = (
        png_chunk(b"gAMA", struct.pack(">I", 45455))
        + png_chunk(b"eXIf", exif_blob("II"))
        + png_chunk(b"tEXt", b"Author\0" + ARTIST.encode())
        + png_chunk(b"tEXt", b"Software\0" + SOFTWARE.encode())
        + png_chunk(b"zTXt", b"Comment\0\0" + zlib.compress(COMMENT.encode()))
        + png_chunk(b"iTXt", xmp)
        + png_chunk(b"tIME", struct.pack(">HBBBBB", 2019, 7, 14, 10, 30, 0))
    )
    return png[:idat_start] + extra + png[idat_start:]


def riff_chunk(kind, data):
    pad = b"\0" if len(data) % 2 else b""
    return kind + struct.pack("<I", len(data)) + data + pad


def make_webp(tmp):
    png_path = os.path.join(tmp, "base.png")
    webp_path = os.path.join(tmp, "base.webp")
    with open(png_path, "wb") as f:
        f.write(base_png())
    subprocess.run(["cwebp", "-quiet", "-q", "80", png_path, "-o", webp_path], check=True)
    with open(webp_path, "rb") as f:
        simple = f.read()
    assert simple[:4] == b"RIFF" and simple[8:12] == b"WEBP"
    image_chunk = simple[12:]
    flags = 0x08 | 0x04  # EXIF and XMP present
    vp8x = struct.pack("<I", flags) + (16 - 1).to_bytes(3, "little") + (16 - 1).to_bytes(3, "little")
    body = (
        b"WEBP"
        + riff_chunk(b"VP8X", vp8x)
        + image_chunk
        + riff_chunk(b"EXIF", exif_blob("II"))
        + riff_chunk(b"XMP ", xmp_packet())
    )
    return b"RIFF" + struct.pack("<I", len(body)) + body


def make_heic(tmp, jpeg):
    src = os.path.join(tmp, "with-exif.jpg")
    dst = os.path.join(tmp, "out.heic")
    with open(src, "wb") as f:
        f.write(jpeg)
    subprocess.run(
        ["sips", "-s", "format", "heic", src, "--out", dst], check=True, capture_output=True
    )
    with open(dst, "rb") as f:
        return f.read()


def pdf_string(text):
    return "(" + text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)") + ")"


def make_pdf(jpeg):
    xmp = xmp_packet()
    content = b"BT /F1 12 Tf 20 60 Td (Quarterly report) Tj ET q 32 0 0 32 20 10 cm /Im1 Do Q"
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R"
        b" /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 7 0 R >> >> >>",
        b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Metadata /Subtype /XML /Length %d >>\nstream\n" % len(xmp) + xmp + b"\nendstream",
        b"<< /Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceRGB"
        b" /BitsPerComponent 8 /Filter /DCTDecode /Length %d >>\nstream\n" % len(jpeg)
        + jpeg
        + b"\nendstream",
        (
            "<< /Title (Quarterly report) /Author "
            + pdf_string(ARTIST)
            + " /Creator "
            + pdf_string(SOFTWARE)
            + " /Producer "
            + pdf_string(MAKE + " PDF Library")
            + " /CreationDate (D:20190714103000Z) /ModDate (D:20190714103000Z)"
            + " /Company "
            + pdf_string(COMPANY)
            + " >>"
        ).encode(),
    ]
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % number + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    for offset in offsets:
        out += b"%010d 00000 n \n" % offset
    out += b"trailer\n<< /Size %d /Root 1 0 R /Info 8 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objects) + 1,
        xref,
    )
    return bytes(out)


CORE_XML = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>Q3 plan</dc:title><dc:creator>{ARTIST}</dc:creator><cp:lastModifiedBy>{ARTIST}</cp:lastModifiedBy><cp:revision>7</cp:revision><cp:lastPrinted>{DATE_ISO}</cp:lastPrinted><dcterms:created xsi:type="dcterms:W3CDTF">{DATE_ISO}</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">{DATE_ISO}</dcterms:modified></cp:coreProperties>"""

APP_XML = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Template>Normal.dotm</Template><TotalTime>42</TotalTime><Application>{SOFTWARE}</Application><Company>{COMPANY}</Company><Manager>{ARTIST}</Manager><AppVersion>16.0000</AppVersion></Properties>"""

CUSTOM_XML = f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><property fmtid="{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}" pid="2" name="ClientName"><vt:lpwstr>{COMPANY}</vt:lpwstr></property></Properties>"""

ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="{main}"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties" Target="docProps/custom.xml"/></Relationships>"""


def content_types(main_override, extra=""):
    return f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="jpeg" ContentType="image/jpeg"/>{main_override}{extra}<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/><Override PartName="/docProps/custom.xml" ContentType="application/vnd.openxmlformats-officedocument.custom-properties+xml"/></Types>"""


def write_zip(parts):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts:
            info = zipfile.ZipInfo(name, date_time=(2019, 7, 14, 10, 30, 0))
            info.compress_type = zipfile.ZIP_STORED if name.endswith(".jpeg") else zipfile.ZIP_DEFLATED
            z.writestr(info, data)
    return buf.getvalue()


def make_docx(jpeg):
    document = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Quarterly plan</w:t></w:r></w:p></w:body></w:document>"""
    doc_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.jpeg"/></Relationships>"""
    main = '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
    return write_zip(
        [
            ("[Content_Types].xml", content_types(main)),
            ("_rels/.rels", ROOT_RELS.format(main="word/document.xml")),
            ("word/document.xml", document),
            ("word/_rels/document.xml.rels", doc_rels),
            ("word/media/image1.jpeg", jpeg),
            ("docProps/core.xml", CORE_XML),
            ("docProps/app.xml", APP_XML),
            ("docProps/custom.xml", CUSTOM_XML),
        ]
    )


def make_xlsx():
    workbook = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"""
    wb_rels = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"""
    sheet = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>42</v></c></row></sheetData></worksheet>"""
    main = '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
    extra = '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
    return write_zip(
        [
            ("[Content_Types].xml", content_types(main, extra)),
            ("_rels/.rels", ROOT_RELS.format(main="xl/workbook.xml")),
            ("xl/workbook.xml", workbook),
            ("xl/_rels/workbook.xml.rels", wb_rels),
            ("xl/worksheets/sheet1.xml", sheet),
            ("docProps/core.xml", CORE_XML),
            ("docProps/app.xml", APP_XML),
            ("docProps/custom.xml", CUSTOM_XML),
        ]
    )


def make_pptx():
    # Only the parts metascrub reads; not a presentation PowerPoint will open.
    presentation = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"/>"""
    main = '<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>'
    return write_zip(
        [
            ("[Content_Types].xml", content_types(main)),
            ("_rels/.rels", ROOT_RELS.format(main="ppt/presentation.xml")),
            ("ppt/presentation.xml", presentation),
            ("docProps/core.xml", CORE_XML),
            ("docProps/app.xml", APP_XML),
            ("docProps/custom.xml", CUSTOM_XML),
        ]
    )


def main():
    tmp = tempfile.mkdtemp()
    try:
        jpeg = make_jpeg(tmp)
        outputs = {
            "photo.jpg": jpeg,
            "photo.png": make_png(),
            "photo.webp": make_webp(tmp),
            "photo.heic": make_heic(tmp, jpeg),
            "report.pdf": make_pdf(jpeg[: -len(TRAILER)]),
            "memo.docx": make_docx(jpeg),
            "budget.xlsx": make_xlsx(),
            "deck.pptx": make_pptx(),
        }
    finally:
        shutil.rmtree(tmp)
    for name, data in outputs.items():
        with open(os.path.join(HERE, name), "wb") as f:
            f.write(data)
        print(f"{name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
