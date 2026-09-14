import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import { Workbook, SpreadsheetFile, Presentation, PresentationFile } from '@oai/artifact-tool';
import * as jsx from '@oai/artifact-tool/presentation-jsx';

const output = 'docs/plugin-artifact-validation';
await fs.mkdir(output, { recursive: true });
const workbook = Workbook.create();
const sheet = workbook.worksheets.add('Validation');
sheet.getRange('A1:B3').values = [['Item', 'Value'], ['Input', 21], ['Computed', null]];
sheet.getRange('B3').formulas = [['=B2*2']];
sheet.getRange('A1:B1').format = { fill: '#173B63', font: { bold: true, color: '#FFFFFF' } };
const xlsx = await SpreadsheetFile.exportXlsx(workbook);
await xlsx.save(`${output}/validated-workbook.xlsx`);
const results = { artifactImport: true, presentationJsxImport: Object.keys(jsx).length > 0, workbookExport: true };
try {
  if (process.argv.includes('--no-render')) throw new Error('Rendering skipped by explicit smoke-test option.');
  const preview = await workbook.render({ sheetName: 'Validation', range: 'A1:B3', scale: 1, format: 'png' });
  await fs.writeFile(`${output}/workbook.png`, new Uint8Array(await preview.arrayBuffer()));
  results.workbookRender = true;
} catch (error) { results.workbookRender = false; results.workbookRenderError = error.message.slice(0, 600); }
const deck = Presentation.create({ slideSize: { width: 1280, height: 720 } });
const slide = deck.slides.add();
slide.background.fill = '#FFFFFF';
const title = slide.shapes.add({ geometry: 'rect', position: { left: 60, top: 60, width: 1120, height: 120 } });
title.text = 'k-Coder artifact runtime';
title.text.fontSize = 40;
const pptx = await PresentationFile.exportPptx(deck);
await pptx.save(`${output}/validated-presentation.pptx`);
results.presentationExport = true;
try {
  if (process.argv.includes('--no-render')) throw new Error('Rendering skipped by explicit smoke-test option.');
  const preview = await deck.export({ slide, format: 'png', scale: 1 });
  await fs.writeFile(`${output}/presentation.png`, new Uint8Array(await preview.arrayBuffer()));
  results.presentationRender = true;
} catch (error) { results.presentationRender = false; results.presentationRenderError = error.message.slice(0, 600); }
assert(results.presentationJsxImport);
await fs.writeFile(`${output}/${process.argv.includes('--no-render') ? 'node-export-results' : 'node-render-results'}.json`, JSON.stringify(results, null, 2));
console.log(JSON.stringify(results, null, 2));
