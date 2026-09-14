"""Generate and reopen real OOXML artifacts; explicitly separate layout QA."""
import json
from pathlib import Path
from zipfile import ZipFile
from docx import Document
from openpyxl import load_workbook
from pptx import Presentation

output = Path('docs/plugin-artifact-validation')
output.mkdir(parents=True, exist_ok=True)
doc = Document()
doc.add_heading('k-Coder 文档插件验证', 0)
doc.add_paragraph('文档由本机已安装 Python 运行时生成，包含可编辑中文段落。')
table = doc.add_table(rows=1, cols=2)
table.rows[0].cells[0].text = '能力'
table.rows[0].cells[1].text = '验证'
table.add_row().cells[0].text = 'DOCX 保存与重开'
table.rows[1].cells[1].text = '通过'
path = output / 'validated-document.docx'
doc.save(path)
reopened = Document(path)
assert '文档插件验证' in reopened.paragraphs[0].text
assert reopened.tables[0].cell(1, 1).text == '通过'
wb = load_workbook(output / 'validated-workbook.xlsx', data_only=False)
assert wb['Validation']['B3'].value == '=B2*2'
values = load_workbook(output / 'validated-workbook.xlsx', data_only=True)
assert values['Validation']['B3'].value == 42
deck = Presentation(output / 'validated-presentation.pptx')
assert len(deck.slides) == 1
assert any('k-Coder artifact runtime' in shape.text for shape in deck.slides[0].shapes if shape.has_text_frame)
for filename in ['validated-document.docx', 'validated-workbook.xlsx', 'validated-presentation.pptx']:
    with ZipFile(output / filename) as archive:
        assert archive.testzip() is None
result = dict(docxReopen=True, xlsxFormulaPreserved=True, xlsxCachedValue=42, pptxEditableText=True, ooxmlZipIntegrity=True, docxVisualQA=False)
(output / 'python-results.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
print(json.dumps(result))
