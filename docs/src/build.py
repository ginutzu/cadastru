"""Construiește un PDF din HTML: copertă fără subsol + corp cu subsol, cuprins cu numere de
pagină (din semnele de carte generate de Chromium) și metadate.

python3 build.py sursa.html iesire.pdf "Titlu subsol"
Sursa conține <section class="cover">…</section> și restul documentului; cuprinsul este
<nav id="toc"></nav> și se completează automat din titlurile h2/h3 cu atributul id.
"""
import sys, re, io, html
from pathlib import Path
from playwright.sync_api import sync_playwright
from pypdf import PdfReader, PdfWriter

src, out, foot = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
doc = src.read_text(encoding='utf-8')
doc = re.sub(r'<table>(\s*)(<tr><th.*?</tr>)', r'<table>\1<thead>\2</thead>', doc, flags=re.S)
m = re.search(r'(<section class="cover">.*?</section>)', doc, re.S)
cover_html = doc.replace(doc[doc.index('<body>')+6:doc.index('</body>')], m.group(1))
cover_html = cover_html.replace('</head>', '<style>@page{margin:0}</style></head>', 1)
body_html = doc.replace(m.group(1), '')

heads = re.findall(r'<h([23]) id="([^"]+)">(.*?)</h\1>', body_html, re.S)

def toc_html(pages):
    rows = []
    for lvl, hid, txt in heads:
        mnum = re.match(r'\s*<span class="n">(.*?)</span>(.*)', txt, re.S)
        num, rest = (mnum.group(1), mnum.group(2)) if mnum else ('', txt)
        t = re.sub(r'<[^>]+>', '', rest).strip()
        p = pages.get(hid, '')
        rows.append(f'<a class="toc{lvl}" href="#{hid}"><span class="tn">{num}</span><span class="t">{t}</span><span class="d"></span><span class="p">{p}</span></a>')
    return '<nav id="toc"><div class="toch">Cuprins</div>' + ''.join(rows) + '</nav>'

FOOT = f'''<div style="font-family:Inter,sans-serif;font-size:7.5pt;color:#6b7280;width:100%;
padding:0 18mm;display:flex;justify-content:space-between;">
<span>{html.escape(foot)}</span><span>pagina <span class="pageNumber"></span> din <span class="totalPages"></span></span></div>'''

def render(pg, content, footer):
    tmp = src.parent / ('_tmp_' + src.name)
    tmp.write_text(content, encoding='utf-8')
    pg.goto(tmp.resolve().as_uri(), wait_until='networkidle')
    pg.wait_for_timeout(300)
    tmp.unlink()
    opts = dict(format='A4', print_background=True, prefer_css_page_size=True, outline=True, tagged=True)
    if footer:
        opts.update(display_header_footer=True, header_template='<span></span>', footer_template=FOOT)
    return pg.pdf(**opts)

base = src.parent.resolve().as_uri() + '/'
def with_base(s):
    return s.replace('<head>', f'<head><base href="{base}">', 1)

with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page()
    # treapta 1: cuprins cu numere fictive (aceeași lățime) -> paginile titlurilor
    b1 = body_html.replace('<nav id="toc"></nav>', toc_html({h[1]: '00' for h in heads}))
    pdf1 = render(pg, with_base(b1), True)
    r = PdfReader(io.BytesIO(pdf1))
    pages = {}
    def walk(items):
        for it in items:
            if isinstance(it, list):
                walk(it); continue
            title = it.title.strip()
            for lvl, hid, txt in heads:
                t = re.sub(r'<[^>]+>', '', html.unescape(txt)).strip()
                if t == title and hid not in pages:
                    pages[hid] = r.get_destination_page_number(it) + 1
                    break
    walk(r.outline)
    missing = [h[1] for h in heads if h[1] not in pages]
    if missing:
        print('ATENȚIE: titluri fără pagină:', missing)
    # treapta 2: cuprinsul final
    b2 = body_html.replace('<nav id="toc"></nav>', toc_html(pages))
    pdf2 = render(pg, with_base(b2), True)
    pdfc = render(pg, with_base(cover_html), False)
    b.close()

w = PdfWriter()
w.append(PdfReader(io.BytesIO(pdfc)), import_outline=False)
w.append(PdfReader(io.BytesIO(pdf2)), import_outline=False)
ncov = len(PdfReader(io.BytesIO(pdfc)).pages)
parent = None
for lvl, hid, txt in heads:
    if hid not in pages:
        continue
    mnum = re.match(r'\s*<span class="n">(.*?)</span>(.*)', txt, re.S)
    title = (mnum.group(1) + ' ' + re.sub(r'<[^>]+>', '', mnum.group(2)).strip()) if mnum else re.sub(r'<[^>]+>', '', txt).strip()
    title = html.unescape(title)
    idx = ncov + pages[hid] - 1
    if lvl == '2':
        parent = w.add_outline_item(title, idx)
    else:
        w.add_outline_item(title, idx, parent=parent)
title = re.search(r'<title>(.*?)</title>', doc).group(1)
w.add_metadata({'/Title': title, '/Author': 'Ginu Popescu', '/Subject': foot,
                '/Creator': 'Cadastru', '/Producer': 'Cadastru'})
w.page_mode = '/UseOutlines'
with open(out, 'wb') as f:
    w.write(f)
print(out, len(w.pages), 'pagini')
