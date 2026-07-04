// monarch.js -- Monarch tokenizer grammar for Wirescript

export const monarchLanguage = {
  defaultToken: 'invalid',
  ignoreCase: false,

  keywords: [
    'if', 'else', 'then', 'match', 'on', 'return', 'emit', 'await',
    'var', 'array', 'buffer', 'let', 'fn', 'chip', 'mod', 'in', 'out', 'open', 'ref',
    'import', 'from', 'as', 'event', 'static', 'type',
  ],

  typeKeywords: [
    'int', 'float', 'bool', 'string', 'entity', 'controller', 'character',
    'vector', 'rotator', 'color', 'exec', 'brick', 'prefab', 'any', 'never',
  ],

  builtins: [
    'sin', 'cos', 'asin', 'acos', 'atan', 'atan2',
    'sinh', 'cosh', 'tanh', 'asinh', 'acosh', 'atanh',
    'exp', 'ln', 'sign', 'round',
    'Deg2Rad', 'Rad2Deg', 'min', 'max', 'BitCount',
    'MakeColor', 'Vec', 'Dot', 'Cross', 'Normalize',
    'Magnitude', 'MagnitudeSq', 'Distance', 'DistanceSq',
    'ScaleVec', 'RotToDir',
    'DisplayText', 'ControllerOf', 'CharacterOf',
    'GetAim',
    'InputReader', 'Random', 'Fmt',
    'GetLocation', 'GetRotation', 'GetLocationRotation',
    'GetLinearVelocity', 'GetAngularVelocity', 'GetVelocity',
    'SetLocation', 'SetRotation', 'SetLocationRotation', 'AddLocationRotation',
    'Teleport', 'RelativeTeleport',
    'SetVelocity', 'AddVelocity', 'SetLinearVelocity', 'SetAngularVelocity',
    'SetGravityDirection',
    'SetLeaderboard', 'IncLeaderboard', 'GetLeaderboard', 'GetTeam',
    'SpawnPrefab', 'Sweep',
  ],

  events: [
    'RoundStart', 'RoundEnd',
    'CharacterSpawned', 'CharacterDied',
    'ControllerJoined', 'ControllerLeft',
    'ZoneEntered', 'ZoneLeft',
    'BrickChanged', 'BrickRemoved',
    'Bumped',
  ],

  constants: ['true', 'false'],

  operators: [
    '&&', '||', '==', '!=', '<=', '>=', '..', '**',
    '<<', '>>', '->', '=>',
    '+', '-', '*', '/', '<', '>', '!', '&', '|', '^', '~', '%', '=',
  ],

  // Used for matching operator characters
  symbols: /[=><!~?:&|+\-*\/\^%\.]+/,

  // Escape sequences for strings
  escapes: /\\(?:[\\$"'ntr0])/,

  tokenizer: {
    root: [
      // Doc comments (must come before line comments)
      [/\/\/\/.*$/, 'comment.doc'],

      // Line comments
      [/\/\/.*$/, 'comment'],

      // Block comments (with nesting support via @comment state)
      [/\/\*/, 'comment', '@comment'],

      // Whitespace
      [/\s+/, 'white'],

      // Numbers -- hex
      [/0[xX][0-9a-fA-F][0-9a-fA-F_]*/, 'number.hex'],

      // Numbers -- binary
      [/0[bB][01][01_]*/, 'number.binary'],

      // Numbers -- float with exponent
      [/[0-9][0-9_]*\.[0-9][0-9_]*[eE][+-]?[0-9][0-9_]*/, 'number.float'],

      // Numbers -- float with decimal
      [/[0-9][0-9_]*\.[0-9][0-9_]*/, 'number.float'],

      // Numbers -- float with exponent only
      [/[0-9][0-9_]*[eE][+-]?[0-9][0-9_]*/, 'number.float'],

      // Numbers -- integer
      [/[0-9][0-9_]*/, 'number'],

      // Double-quoted strings with interpolation
      [/"/, 'string', '@string_double'],

      // Single-quoted strings with interpolation
      [/'/, 'string', '@string_single'],

      // Identifiers, keywords, types, builtins
      [/[a-zA-Z_]\w*/, {
        cases: {
          '@constants': 'constant',
          '@keywords': 'keyword',
          '@typeKeywords': 'type',
          '@builtins': 'support.function',
          '@events': 'variable.predefined',
          '@default': 'identifier',
        },
      }],

      // Delimiters and operators
      [/[{}()\[\]]/, '@brackets'],
      [/[;,]/, 'delimiter'],

      // Multi-char operators first
      [/@symbols/, {
        cases: {
          '@operators': 'operator',
          '@default': '',
        },
      }],
    ],

    // Nested block comments
    comment: [
      [/\/\*/, 'comment', '@push'],  // nested open
      [/\*\//, 'comment', '@pop'],    // close
      [/./, 'comment'],
    ],

    // Double-quoted string with interpolation
    string_double: [
      [/\$\{/, { token: 'delimiter.bracket', next: '@interpolation' }],
      [/@escapes/, 'string.escape'],
      [/\\/, 'string.escape.invalid'],
      [/"/, 'string', '@pop'],
      [/[^"\\$]+/, 'string'],
      [/\$/, 'string'],
    ],

    // Single-quoted string with interpolation
    string_single: [
      [/\$\{/, { token: 'delimiter.bracket', next: '@interpolation' }],
      [/@escapes/, 'string.escape'],
      [/\\/, 'string.escape.invalid'],
      [/'/, 'string', '@pop'],
      [/[^'\\$]+/, 'string'],
      [/\$/, 'string'],
    ],

    // String interpolation -- re-enter root tokenizer within ${...}
    interpolation: [
      [/\}/, { token: 'delimiter.bracket', next: '@pop' }],
      { include: 'root' },
    ],
  },
};

export const monarchConfiguration = {
  comments: {
    lineComment: '//',
    blockComment: ['/*', '*/'],
  },
  brackets: [
    ['{', '}'],
    ['[', ']'],
    ['(', ')'],
  ],
  autoClosingPairs: [
    { open: '{', close: '}' },
    { open: '[', close: ']' },
    { open: '(', close: ')' },
    { open: '"', close: '"', notIn: ['string'] },
    { open: "'", close: "'", notIn: ['string'] },
    { open: '/*', close: '*/', notIn: ['string'] },
  ],
  surroundingPairs: [
    { open: '{', close: '}' },
    { open: '[', close: ']' },
    { open: '(', close: ')' },
    { open: '"', close: '"' },
    { open: "'", close: "'" },
  ],
  folding: {
    markers: {
      start: /\{/,
      end: /\}/,
    },
  },
  indentationRules: {
    increaseIndentPattern: /\{\s*$/,
    decreaseIndentPattern: /^\s*\}/,
  },
};
