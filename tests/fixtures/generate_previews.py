"""Synthetic PDF preview fixtures; run with ReportLab. No account content."""
from pathlib import Path
from reportlab.pdfgen.canvas import Canvas
root = Path(__file__).parent
for name, size, rotation in [('portrait',(768,1024),0),('landscape',(1024,768),0),('rotated',(768,1024),90)]:
    c = Canvas(str(root / f'preview-{name}.pdf'), pagesize=size, invariant=1)
    c.setTitle('Heya synthetic preview test')
    c.setAuthor('Heya tests')
    c.setPageRotation(rotation)
    w,h=size
    c.setFillColorRGB(1,1,1); c.rect(0,0,w,h,fill=1,stroke=0)
    c.setFillColorRGB(0,0,0); c.setFont('Helvetica-Bold',25)
    c.drawString(24,h-42,'HEYA - SYNTHETIC DOCUMENT')
    c.setFont('Helvetica',13); c.drawString(24,h-69,'First-page preview: fine text, table lines and vector detail')
    c.setLineWidth(0.5); c.rect(24,h-190,w-48,95,fill=0,stroke=1)
    for i in range(1,4): c.line(24,h-95-i*23,w-24,h-95-i*23)
    c.setFont('Helvetica',10)
    for i,text in enumerate(['Item / Description                               Quantity         Amount', 'Preview quality / fine print                    2                123.45', 'All data in this file is synthetic               1                 67.89']):
        c.drawString(32,h-112-i*23,text)
    c.setFillColorRGB(0,0,0)
    for x in range(24,min(w-24,600),2): c.rect(x,h-245,1,24,stroke=0,fill=1)
    c.setFont('Helvetica',10); c.drawString(24,h-265,'Alternating one-point rules: keep the detail, do not enlarge a tiny bitmap.')
    c.setFillColorRGB(1,0,0); c.rect(0,0,w,200,fill=1,stroke=0)
    c.showPage()
    c.setFillColorRGB(1,0,0); c.rect(0,0,w,h,fill=1,stroke=0)
    c.showPage(); c.save()
