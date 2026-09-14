// Record structure only. Typed values and navigation URLs are parameters, never
// durable recording data. This module does not execute an agent or grant access.
export async function createRecording(context) {
  const toolGraph = [];
  const variables = {};
  let inputNumber = 0;
  let urlNumber = 0;
  let requiresManualSensitiveInput = false;
  let truncated = false;
  function append(toolName, argsTemplate) {
    if (toolGraph.length >= 512) { truncated = true; return; }
    toolGraph.push({ stepId: `step_${toolGraph.length + 1}`, toolName, argsTemplate });
  }
  await context.exposeBinding('__kcoderRecordAction', (source, event) => {
    if (source.frame !== source.page.mainFrame() || !event || typeof event !== 'object') return;
    if (event.kind === 'sensitive') { requiresManualSensitiveInput = true; return; }
    if (!['input', 'click'].includes(event.kind) || typeof event.selector !== 'string' || event.selector.length > 1000) return;
    if (event.kind === 'input') {
      const previous = toolGraph.at(-1);
      if (previous?.toolName === 'browser_type' && previous.argsTemplate.selector === event.selector) return;
      const name = `input_${++inputNumber}`;
      variables[name] = { description: 'Input value supplied at replay time; never put credentials in a workflow file.' };
      append('browser_type', { selector: event.selector, text: `{{${name}}}` });
    } else {
      append('browser_click', { selector: event.selector });
    }
  });
  await context.addInitScript(() => {
    function selector(element) {
      if (element.id) return `#${CSS.escape(element.id)}`;
      if (element.getAttribute('data-testid')) return `[data-testid="${CSS.escape(element.getAttribute('data-testid'))}"]`;
      if (element.getAttribute('name')) return `${element.tagName.toLowerCase()}[name="${CSS.escape(element.getAttribute('name'))}"]`;
      const parts = [];
      for (let node = element; node && node !== document.body; node = node.parentElement) {
        const siblings = [...node.parentElement.children].filter(child => child.tagName === node.tagName);
        parts.unshift(`${node.tagName.toLowerCase()}:nth-of-type(${siblings.indexOf(node) + 1})`);
      }
      return `body > ${parts.join(' > ')}`;
    }
    document.addEventListener('input', event => {
      const target = event.target;
      if (!(target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)) return;
      const attributes = ['type', 'name', 'id', 'autocomplete', 'aria-label'].map(name => target.getAttribute(name) || '').join(' ');
      if (/password|passcode|token|secret|api.?key|authorization|credit.?card|cc-number|密码|密钥/i.test(attributes)) {
        void window.__kcoderRecordAction({ kind: 'sensitive' });
      } else {
        // Do not read target.value, even for fields that appear non-sensitive.
        void window.__kcoderRecordAction({ kind: 'input', selector: selector(target) });
      }
    }, true);
    document.addEventListener('click', event => {
      const target = event.target instanceof Element ? event.target.closest('button,a,input[type=submit],[role=button]') : null;
      if (target) void window.__kcoderRecordAction({ kind: 'click', selector: selector(target) });
    }, true);
  });
  const trackPage = page => page.on('framenavigated', frame => {
    if (frame !== page.mainFrame() || !/^https?:\/\//.test(frame.url())) return;
    const name = `url_${++urlNumber}`;
    variables[name] = { description: 'Navigation URL supplied at replay time; the original URL was not recorded.' };
    append('browser_navigate', { url: `{{${name}}}` });
  });
  context.on('page', trackPage);
  for (const page of context.pages()) trackPage(page);
  return {
    snapshot: () => structuredClone({
      name: 'recorded-browser-workflow', objective: 'Replay the demonstrated browser actions with explicitly supplied parameters.',
      variables, toolGraph, requiresManualSensitiveInput, truncated,
    }),
  };
}
