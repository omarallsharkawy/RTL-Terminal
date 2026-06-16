// Validation harness for the Arabic shaping engine in src/components/XtermTerminal.tsx.
// Mirrors the exact ARABIC_MAP entries + algorithm so we can assert correct output
// without a GUI. Run: node scripts/test-shaping.mjs
const M = {
  '\u0628': { isolated: '\uFE8F', final: '\uFE90', initial: '\uFE91', medial: '\uFE92', joining: 'dual' },
  '\u0627': { isolated: '\uFE8D', final: '\uFE8E', joining: 'right' },
  '\u0631': { isolated: '\uFEAD', final: '\uFEAE', joining: 'right' },
  '\u062D': { isolated: '\uFEA1', final: '\uFEA2', initial: '\uFEA3', medial: '\uFEA4', joining: 'dual' },
  '\u0645': { isolated: '\uFEE1', final: '\uFEE2', initial: '\uFEE3', medial: '\uFEE4', joining: 'dual' },
  '\u0644': { isolated: '\uFEDD', final: '\uFEDE', initial: '\uFEDF', medial: '\uFEE0', joining: 'dual' },
  '\u0639': { isolated: '\uFEC9', final: '\uFECA', initial: '\uFECB', medial: '\uFECC', joining: 'dual' },
  '\u064A': { isolated: '\uFEF1', final: '\uFEF2', initial: '\uFEF3', medial: '\uFEF4', joining: 'dual' },
  '\u0646': { isolated: '\uFEE5', final: '\uFEE6', initial: '\uFEE7', medial: '\uFEE8', joining: 'dual' },
};

function shape(word) {
  const chars = [...word];
  return chars.map((ch, i) => {
    const e = M[ch];
    if (!e) return ch;
    const pe = M[chars[i - 1]];
    const ne = M[chars[i + 1]];
    const joinsPrev = !!pe && pe.joining === 'dual' && e.joining !== 'none';
    const joinsNext = e.joining === 'dual' && !!ne && ne.joining !== 'none';
    if (joinsPrev && joinsNext) return e.medial || e.isolated;
    if (joinsPrev) return e.final || e.isolated;
    if (joinsNext) return e.initial || e.isolated;
    return e.isolated;
  }).join('');
}

const hex = (s) => [...s].map((c) => c.codePointAt(0).toString(16).toUpperCase().padStart(4, '0')).join(' ');

const cases = [
  // مرحبا  Meem(initial) Ra(final, right-joining breaks the chain) Hah(initial) Beh(medial) Alef(final)
  ['marhaba', '\u0645\u0631\u062D\u0628\u0627', 'FEE3 FEAE FEA3 FE92 FE8E'],
  // علي  Ain(dual,initial) Lam(dual,medial) Yeh(dual,final)
  ['ali',     '\u0639\u0644\u064A',             'FECB FEE0 FEF2'],
  // بين  Beh(dual,initial) Yeh(dual,medial) Noon(dual,final)
  ['bayn',    '\u0628\u064A\u0646',             'FE91 FEF4 FEE6'],
];

let pass = 0;
for (const [name, input, expect] of cases) {
  const out = hex(shape(input));
  const ok = out === expect;
  // Every output codepoint must be a presentation form (U+FE70..FEFF) since all letters join.
  const allForms = [...shape(input)].every((c) => c.codePointAt(0) >= 0xFE70 && c.codePointAt(0) <= 0xFEFF);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name.padEnd(8)} got=[${out}] forms=${allForms}`);
  if (ok && allForms) pass++;
}
console.log(`\n${pass}/${cases.length} cases passed`);
process.exit(pass === cases.length ? 0 : 1);
