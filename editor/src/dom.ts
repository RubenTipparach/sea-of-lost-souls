// Tiny DOM helpers so the panel stays framework-free but readable.

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Partial<Record<string, string>> = {},
  children: (Node | string)[] = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined) continue;
    if (k === 'class') node.className = v;
    else node.setAttribute(k, v);
  }
  for (const child of children) {
    node.append(typeof child === 'string' ? document.createTextNode(child) : child);
  }
  return node;
}

export function section(title: string, body: HTMLElement): HTMLElement {
  return el('div', { class: 'section' }, [el('h2', {}, [title]), body]);
}

function field(label: string, input: HTMLElement): HTMLElement {
  return el('div', { class: 'field' }, [el('label', {}, [label]), input]);
}

export function textField(
  label: string,
  value: string,
  onChange: (v: string) => void,
): HTMLElement {
  const input = el('input', { type: 'text', value });
  input.addEventListener('input', () => onChange(input.value));
  return field(label, input);
}

export function numberField(
  label: string,
  value: number,
  onChange: (v: number) => void,
  step = 'any',
): HTMLElement {
  const input = el('input', { type: 'number', value: String(value), step });
  input.addEventListener('input', () => {
    const n = Number(input.value);
    if (!Number.isNaN(n)) onChange(n);
  });
  return field(label, input);
}

export function selectField(
  label: string,
  value: string,
  options: string[],
  onChange: (v: string) => void,
): HTMLElement {
  const select = el(
    'select',
    {},
    options.map((opt) => el('option', { value: opt }, [opt])),
  );
  select.value = value;
  select.addEventListener('change', () => onChange(select.value));
  return field(label, select);
}

export function downloadBlob(filename: string, blob: Blob): void {
  const url = URL.createObjectURL(blob);
  const a = el('a', { href: url, download: filename });
  document.body.appendChild(a);
  a.click();
  a.remove();
  // Defer revoke so the download has a chance to start.
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
