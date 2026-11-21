import { describe, expect, it } from 'vitest';
import {
  EXTRA_SUFFIXES_BY_ICON_KEY,
  ICON_BY_KEY,
  STEMS_BY_ICON_KEY,
  SUFFIXES_BY_ICON_KEY,
  fileIconOf,
} from './fileIcons';

describe('fileIconOf', () => {
  it('matches suffixes before the stem and whole names before suffixes', () => {
    const names = ['mod.rs', 'Dockerfile.rs', 'go.mod', 'Dockerfile', 'Dockerfile.prod', 'docker-compose.yml', '.dockerignore', 'Podfile.lock', 'Chart.yaml', '.gitignore'];
    expect(names.map(name => fileIconOf(name))).toEqual(['rust', 'rust', 'go', 'docker', 'docker', 'docker', 'docker', 'lock', 'helm', 'git']);
  });

  it('ignores case', () => {
    const names = ['IMG_0001.JPG', 'NOTES.TXT', 'START.BAT', 'Main.RS', 'DOCKERFILE', 'clip.MP4'];
    expect(names.map(name => fileIconOf(name))).toEqual(['image', 'book', 'terminal', 'rust', 'docker', 'video']);
  });

  it('draws the forensic categories before the name table', () => {
    const names = ['SETUP.EXE', 'kernel32.dll', 'pagefile.sys', 'evidence.zip', 'backup.tar.gz', 'contacts.csv', 'Book1.xlsx', 'msgstore.db', 'data.sqlite', 'inbox.eml', 'outlook.pst'];
    expect(names.map(name => fileIconOf(name))).toEqual(['binary', 'binary', 'binary', 'archive', 'archive', 'table', 'table', 'database', 'database', 'envelope', 'envelope']);
  });

  it('gives text, logs, configuration and scripts their own icons', () => {
    const names = ['README.md', 'report.pdf', 'server.log', 'php.ini', 'app.cfg', '.env', 'run.sh', 'index.php', 'config.json', 'settings.json5', 'font.TTF', 'NTUSER.DAT'];
    expect(names.map(name => fileIconOf(name))).toEqual(['book', 'book', 'info', 'settings', 'settings', 'settings', 'terminal', 'php', 'code', 'code', 'font', 'database']);
  });

  it('falls back to the category, then to the generic file icon', () => {
    const names = ['unknown', 'shell.jsp', 'old.bak', 'readme.', 'photo.svg', 'track.m4a', 'slides.key'];
    expect(names.map(name => fileIconOf(name))).toEqual(['file', 'file', 'file', 'file', 'image', 'audio', 'book']);
  });
});

describe('file icon tables', () => {
  const tables = [...STEMS_BY_ICON_KEY, ...SUFFIXES_BY_ICON_KEY, ...EXTRA_SUFFIXES_BY_ICON_KEY];

  it('gives every icon key an icon, except keys that deliberately have none', () => {
    const missing = tables.map(([key]) => key).filter(key => !(key in ICON_BY_KEY));
    expect([...new Set(missing)]).toEqual(['backup']);
  });

  it('uses every icon key', () => {
    const used = new Set(tables.map(([key]) => key));
    expect(Object.keys(ICON_BY_KEY).filter(key => !used.has(key))).toEqual([]);
  });

  it('adds suffixes without shadowing the base tables', () => {
    const base = new Set([...STEMS_BY_ICON_KEY, ...SUFFIXES_BY_ICON_KEY].flatMap(([, names]) => names.map(name => name.toLowerCase())));
    const shadowed = EXTRA_SUFFIXES_BY_ICON_KEY.flatMap(([, names]) => names).filter(name => base.has(name.toLowerCase()));
    expect(shadowed).toEqual([]);
  });

  it('keeps one icon key per name after lower-casing', () => {
    for (const table of [STEMS_BY_ICON_KEY, [...SUFFIXES_BY_ICON_KEY, ...EXTRA_SUFFIXES_BY_ICON_KEY]]) {
      const keys = new Map<string, Set<string>>();
      for (const [key, names] of table) {
        for (const name of names) keys.set(name.toLowerCase(), (keys.get(name.toLowerCase()) ?? new Set()).add(key));
      }
      expect([...keys].filter(([, owners]) => owners.size > 1)).toEqual([]);
    }
  });
});
