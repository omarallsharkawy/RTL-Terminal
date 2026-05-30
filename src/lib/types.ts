export type Rgb = [number, number, number];

export type CellStyle = {
  fg?: Rgb;
  bg?: Rgb;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  inverse?: boolean;
};

export type VisualCell = {
  ch: string;
  logicalIndex: number;
  width: number;
  style: CellStyle;
};

export type TerminalLine = {
  row: number;
  baseDirection: 'rtl' | 'ltr';
  cells: VisualCell[];
};

export type TerminalFrame = {
  cols: number;
  rows: number;
  cursorCol: number;
  cursorRow: number;
  cursorVisible: boolean;
  title: string;
  lines: TerminalLine[];
};

export type ThemeName = 'midnight' | 'emerald' | 'graphite';
