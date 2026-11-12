#!/usr/bin/env python3
"""Regenerate the metadata-laden test fixtures in this directory.

The fixtures are committed, so tests do not need this script. It exists so
anyone can see exactly what metadata each fixture carries and rebuild them.

Requirements: Python 3 (stdlib only), macOS `sips` (JPEG/HEIC encoding) and
`cwebp` (WebP encoding). Run from anywhere:

    python3 tests/fixtures/generate.py

Planted names, devices and identifiers contain one of the strings in MARKERS,
so the tests can assert that cleaned output no longer contains them. Planted
dates are checked through `inspect` instead, since a date string alone is not
distinctive enough to search for.
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
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R /AcroForm << /Fields [10 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R"
        b" /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 7 0 R >> >>"
        b" /Annots [9 0 R 10 0 R] >>",
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
        # A sticky-note comment: /T is its author.
        (
            "<< /Type /Annot /Subtype /Text /Rect [150 60 170 80] /T "
            + pdf_string(ARTIST)
            + " /Contents (Check the figures) /M (D:20190714103000Z)"
            + " /CreationDate (D:20190714103000Z) >>"
        ).encode(),
        # A form field: here /T is the field's name and must be kept.
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (total) /V (42) /Rect [20 70 80 90] >>",
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
    # An incremental update replaces the Info dictionary. The first
    # revision, including its Company entry, stays in the file body where
    # naive tools leave it.
    info_offset = len(out)
    out += (
        "8 0 obj\n<< /Title (Quarterly report v2) /Author "
        + pdf_string(ARTIST)
        + " /Creator "
        + pdf_string(SOFTWARE)
        + " /Producer "
        + pdf_string(MAKE + " PDF Library")
        + " /CreationDate (D:20190714103000Z) /ModDate (D:20190715090000Z) >>\nendobj\n"
    ).encode()
    update_xref = len(out)
    out += b"xref\n0 1\n0000000000 65535 f \n8 1\n%010d 00000 n \n" % info_offset
    out += b"trailer\n<< /Size %d /Root 1 0 R /Info 8 0 R /Prev %d >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objects) + 1,
        xref,
        update_xref,
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


W_NS = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
R_NS = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
XML_HEAD = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
USER_ID = "jane@ExampleCorp.com"


def rels(*targets):
    body = "".join(
        f'<Relationship Id="rId{i}" Type="{kind}" Target="{target}"/>'
        for i, (kind, target) in enumerate(targets, start=1)
    )
    return (
        XML_HEAD
        + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        + body
        + "</Relationships>"
    )


def override(part, content_type):
    return f'<Override PartName="/{part}" ContentType="{content_type}"/>'


def make_docx(jpeg):
    # Revision session IDs (rsid), a tracked insertion and a comment all
    # record who edited the document and when.
    document = (
        XML_HEAD
        + f"<w:document {W_NS}><w:body>"
        + '<w:p w:rsidR="00A1B2C3" w:rsidRDefault="00A1B2C3">'
        + '<w:commentRangeStart w:id="0"/><w:r w:rsidR="00D4E5F6"><w:t>Quarterly plan</w:t></w:r>'
        + f'<w:ins w:id="1" w:author="{ARTIST}" w:date="{DATE_ISO}"><w:r><w:t xml:space="preserve"> and budget</w:t></w:r></w:ins>'
        + '<w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r>'
        + "</w:p></w:body></w:document>"
    )
    comments = (
        XML_HEAD
        + f"<w:comments {W_NS}>"
        + f'<w:comment w:id="0" w:author="{ARTIST}" w:date="{DATE_ISO}" w:initials="JE">'
        + "<w:p><w:r><w:t>Check the figures</w:t></w:r></w:p></w:comment></w:comments>"
    )
    settings = (
        XML_HEAD
        + f'<w:settings {W_NS}><w:rsids><w:rsidRoot w:val="00A1B2C3"/>'
        + '<w:rsid w:val="00A1B2C3"/><w:rsid w:val="00D4E5F6"/></w:rsids></w:settings>'
    )
    people = (
        XML_HEAD
        + '<w15:people xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml">'
        + f'<w15:person w15:author="{ARTIST}"><w15:presenceInfo w15:providerId="AD" w15:userId="{USER_ID}"/></w15:person>'
        + "</w15:people>"
    )
    doc_rels = rels(
        (f"{REL}/image", "media/image1.jpeg"),
        (f"{REL}/comments", "comments.xml"),
        (f"{REL}/settings", "settings.xml"),
        ("http://schemas.microsoft.com/office/2011/relationships/people", "people.xml"),
    )
    wml = "application/vnd.openxmlformats-officedocument.wordprocessingml"
    main = override("word/document.xml", f"{wml}.document.main+xml")
    extra = (
        override("word/comments.xml", f"{wml}.comments+xml")
        + override("word/settings.xml", f"{wml}.settings+xml")
        + override("word/people.xml", f"{wml}.people+xml")
    )
    return write_zip(
        [
            ("[Content_Types].xml", content_types(main, extra)),
            ("_rels/.rels", ROOT_RELS.format(main="word/document.xml")),
            ("word/document.xml", document),
            ("word/_rels/document.xml.rels", doc_rels),
            ("word/comments.xml", comments),
            ("word/settings.xml", settings),
            ("word/people.xml", people),
            ("word/media/image1.jpeg", jpeg),
            ("docProps/core.xml", CORE_XML),
            ("docProps/app.xml", APP_XML),
            ("docProps/custom.xml", CUSTOM_XML),
        ]
    )


SML = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'

VML_NOTE = """<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel"><o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout><v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe"><v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype><v:shape id="_x0000_s1025" type="#_x0000_t202" style="position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;z-index:1;visibility:hidden" fillcolor="#ffffe1" o:insetmode="auto"><v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/><v:path o:connecttype="none"/><v:textbox style="mso-direction-alt:auto"><div style="text-align:left"></div></v:textbox><x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/><x:Anchor>1, 15, 0, 2, 3, 15, 3, 16</x:Anchor><x:AutoFill>False</x:AutoFill><x:Row>0</x:Row><x:Column>0</x:Column></x:ClientData></v:shape></xml>"""


def make_xlsx():
    # absPath records the folder the workbook was last saved in, which on
    # Windows usually includes the user's account name.
    workbook = (
        XML_HEAD
        + f"<workbook {SML} {R_NS}"
        + ' xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x15"'
        + ' xmlns:x15="http://schemas.microsoft.com/office/spreadsheetml/2010/11/main">'
        + '<mc:AlternateContent><mc:Choice Requires="x15">'
        + '<x15ac:absPath xmlns:x15ac="http://schemas.microsoft.com/office/spreadsheetml/2010/11/ac"'
        + f' url="C:\\Users\\{ARTIST}\\Documents\\"/></mc:Choice></mc:AlternateContent>'
        + '<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>'
    )
    wb_rels = rels(
        (f"{REL}/worksheet", "worksheets/sheet1.xml"),
        ("http://schemas.microsoft.com/office/2017/10/relationships/person", "persons/person.xml"),
    )
    sheet = (
        XML_HEAD
        + f"<worksheet {SML} {R_NS}><sheetData><row r=\"1\"><c r=\"A1\"><v>42</v></c></row></sheetData>"
        + '<legacyDrawing r:id="rId2"/></worksheet>'
    )
    sheet_rels = rels(
        (f"{REL}/comments", "../comments1.xml"),
        (f"{REL}/vmlDrawing", "../drawings/vmlDrawing1.vml"),
    )
    # Excel prefixes note text with the author's name in bold.
    comments = (
        XML_HEAD
        + f"<comments {SML}><authors><author>{ARTIST}</author></authors>"
        + '<commentList><comment ref="A1" authorId="0"><text>'
        + f"<r><rPr><b/></rPr><t>{ARTIST}:</t></r><r><t xml:space=\"preserve\">\nCheck this total</t></r>"
        + "</text></comment></commentList></comments>"
    )
    persons = (
        XML_HEAD
        + '<personList xmlns="http://schemas.microsoft.com/office/spreadsheetml/2018/threadedcomments">'
        + f'<person displayName="{ARTIST}" id="{{6C1F3C2E-1D5B-4F5E-9C1A-0A1B2C3D4E5F}}"'
        + f' userId="{USER_ID}" providerId="AD"/></personList>'
    )
    sml = "application/vnd.openxmlformats-officedocument.spreadsheetml"
    main = override("xl/workbook.xml", f"{sml}.sheet.main+xml")
    extra = (
        override("xl/worksheets/sheet1.xml", f"{sml}.worksheet+xml")
        + override("xl/comments1.xml", f"{sml}.comments+xml")
        + override("xl/persons/person.xml", "application/vnd.ms-excel.person+xml")
        + '<Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/>'
    )
    return write_zip(
        [
            ("[Content_Types].xml", content_types(main, extra)),
            ("_rels/.rels", ROOT_RELS.format(main="xl/workbook.xml")),
            ("xl/workbook.xml", workbook),
            ("xl/_rels/workbook.xml.rels", wb_rels),
            ("xl/worksheets/sheet1.xml", sheet),
            ("xl/worksheets/_rels/sheet1.xml.rels", sheet_rels),
            ("xl/comments1.xml", comments),
            ("xl/drawings/vmlDrawing1.vml", VML_NOTE),
            ("xl/persons/person.xml", persons),
            ("docProps/core.xml", CORE_XML),
            ("docProps/app.xml", APP_XML),
            ("docProps/custom.xml", CUSTOM_XML),
        ]
    )


PML_NS = (
    'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
    + R_NS
    + ' xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'
)
EMPTY_TREE = (
    '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>'
)


def theme_xml():
    colours = [
        ("dk1", '<a:sysClr val="windowText" lastClr="000000"/>'),
        ("lt1", '<a:sysClr val="window" lastClr="FFFFFF"/>'),
        ("dk2", '<a:srgbClr val="44546A"/>'),
        ("lt2", '<a:srgbClr val="E7E6E6"/>'),
        ("accent1", '<a:srgbClr val="4472C4"/>'),
        ("accent2", '<a:srgbClr val="ED7D31"/>'),
        ("accent3", '<a:srgbClr val="A5A5A5"/>'),
        ("accent4", '<a:srgbClr val="FFC000"/>'),
        ("accent5", '<a:srgbClr val="5B9BD5"/>'),
        ("accent6", '<a:srgbClr val="70AD47"/>'),
        ("hlink", '<a:srgbClr val="0563C1"/>'),
        ("folHlink", '<a:srgbClr val="954F72"/>'),
    ]
    scheme = "".join(f"<a:{name}>{value}</a:{name}>" for name, value in colours)
    fill = '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>'
    line = f'<a:ln w="6350">{fill}</a:ln>'
    font = '<a:latin typeface="{}"/><a:ea typeface=""/><a:cs typeface=""/>'
    return (
        XML_HEAD
        + '<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office">'
        + f'<a:themeElements><a:clrScheme name="Office">{scheme}</a:clrScheme>'
        + '<a:fontScheme name="Office">'
        + f"<a:majorFont>{font.format('Calibri Light')}</a:majorFont>"
        + f"<a:minorFont>{font.format('Calibri')}</a:minorFont></a:fontScheme>"
        + '<a:fmtScheme name="Office">'
        + f"<a:fillStyleLst>{fill * 3}</a:fillStyleLst>"
        + f"<a:lnStyleLst>{line * 3}</a:lnStyleLst>"
        + f"<a:effectStyleLst>{'<a:effectStyle><a:effectLst/></a:effectStyle>' * 3}</a:effectStyleLst>"
        + f"<a:bgFillStyleLst>{fill * 3}</a:bgFillStyleLst>"
        + "</a:fmtScheme></a:themeElements></a:theme>"
    )


def make_pptx():
    presentation = (
        XML_HEAD
        + f'<p:presentation {PML_NS}>'
        + '<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>'
        + '<p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst>'
        + '<p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/>'
        + "</p:presentation>"
    )
    master = (
        XML_HEAD
        + f"<p:sldMaster {PML_NS}><p:cSld><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld>"
        + '<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2"'
        + ' accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6"'
        + ' hlink="hlink" folHlink="folHlink"/>'
        + '<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst>'
        + "</p:sldMaster>"
    )
    layout = (
        XML_HEAD
        + f'<p:sldLayout {PML_NS} type="blank" preserve="1">'
        + f'<p:cSld name="Blank"><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld>'
        + "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"
    )
    slide = (
        XML_HEAD
        + f"<p:sld {PML_NS}><p:cSld><p:spTree>{EMPTY_TREE}"
        + '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr>'
        + '<p:spPr><a:xfrm><a:off x="914400" y="914400"/><a:ext cx="7315200" cy="914400"/></a:xfrm>'
        + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
        + '<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="4000"/>'
        + "<a:t>Quarterly deck</a:t></a:r></a:p></p:txBody></p:sp>"
        + "</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
    )
    authors = (
        XML_HEAD
        + '<p:cmAuthorLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
        + f'<p:cmAuthor id="0" name="{ARTIST}" initials="JE" lastIdx="1" clrIdx="0"/></p:cmAuthorLst>'
    )
    comments = (
        XML_HEAD
        + '<p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">'
        + '<p:cm authorId="0" dt="2019-07-14T10:30:00.000" idx="1"><p:pos x="10" y="10"/>'
        + "<p:text>Tighten this title</p:text></p:cm></p:cmLst>"
    )
    pml = "application/vnd.openxmlformats-officedocument.presentationml"
    main = override("ppt/presentation.xml", f"{pml}.presentation.main+xml")
    extra = (
        override("ppt/slideMasters/slideMaster1.xml", f"{pml}.slideMaster+xml")
        + override("ppt/slideLayouts/slideLayout1.xml", f"{pml}.slideLayout+xml")
        + override("ppt/slides/slide1.xml", f"{pml}.slide+xml")
        + override("ppt/theme/theme1.xml", "application/vnd.openxmlformats-officedocument.theme+xml")
        + override("ppt/commentAuthors.xml", f"{pml}.commentAuthors+xml")
        + override("ppt/comments/comment1.xml", f"{pml}.comments+xml")
    )
    return write_zip(
        [
            ("[Content_Types].xml", content_types(main, extra)),
            ("_rels/.rels", ROOT_RELS.format(main="ppt/presentation.xml")),
            ("ppt/presentation.xml", presentation),
            (
                "ppt/_rels/presentation.xml.rels",
                rels(
                    (f"{REL}/slideMaster", "slideMasters/slideMaster1.xml"),
                    (f"{REL}/slide", "slides/slide1.xml"),
                    (f"{REL}/theme", "theme/theme1.xml"),
                    (f"{REL}/commentAuthors", "commentAuthors.xml"),
                ),
            ),
            ("ppt/slideMasters/slideMaster1.xml", master),
            (
                "ppt/slideMasters/_rels/slideMaster1.xml.rels",
                rels(
                    (f"{REL}/slideLayout", "../slideLayouts/slideLayout1.xml"),
                    (f"{REL}/theme", "../theme/theme1.xml"),
                ),
            ),
            ("ppt/slideLayouts/slideLayout1.xml", layout),
            (
                "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
                rels((f"{REL}/slideMaster", "../slideMasters/slideMaster1.xml")),
            ),
            ("ppt/slides/slide1.xml", slide),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                rels(
                    (f"{REL}/slideLayout", "../slideLayouts/slideLayout1.xml"),
                    (f"{REL}/comments", "../comments/comment1.xml"),
                ),
            ),
            ("ppt/theme/theme1.xml", theme_xml()),
            ("ppt/commentAuthors.xml", authors),
            ("ppt/comments/comment1.xml", comments),
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
