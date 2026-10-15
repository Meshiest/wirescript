import { test } from 'node:test';
import assert from 'node:assert/strict';
import { adminGatesIn, benchMismatch, blockRules, inlinePrefabs, sidePins, unknownAssets } from '../check_learn.mjs';
import { wsBlocks } from '../lib.mjs';

test('reads the rules from a fence info string', () => {
  const [b] = wsBlocks('```wirescript name=door, bench-for=lock\nin a: bool\n```\n');
  assert.deepEqual(blockRules(b.words), {
    ignore: false,
    nocompile: false,
    admin: false,
    name: 'door',
    expect: null,
    benchFor: 'lock',
  });
  const [e] = wsBlocks('```wirescript name=bad expect=WSP001\nin on: bool\n```\n');
  assert.equal(blockRules(e.words).expect, 'WSP001');
  const [d] = wsBlocks('```wirescript name=debug admin\nx\n```\n');
  assert.equal(blockRules(d.words).admin, true);
  const [i] = wsBlocks('```wirescript ignore\nx\n```\n');
  assert.equal(blockRules(i.words).ignore, true);
});

test('keeps the hidden prelude with its block', () => {
  const md = '<!-- doc-check-prelude:\nin go: exec\n-->\n\n```wirescript name=a\non go {}\n```\n';
  const [b] = wsBlocks(md);
  assert.equal(b.prelude, 'in go: exec\n');
  assert.equal(b.code, 'on go {}');
});

const ir = [
  'module @x {',
  '  %a = in bool { side = left } loc(1:7+6 to 1:17+16)',
  '  %go = in exec loc(2:1+0 to 2:5+4)',
  '  out %b bool = %a { side = right } loc(2:8+24 to 2:17+33)',
  '  out %done exec { side = right } loc(3:1+0 to 3:5+4)',
  '}',
].join('\n');

test('reads side pins from the IR and ignores unsided ports', () => {
  assert.deepEqual(sidePins(ir), { inputs: ['a:bool@left'], outputs: ['b:bool@right', 'done:exec@right'] });
});

test('a bench must mirror its subject pin for pin', () => {
  const subject = { inputs: ['a:bool@left', 'b:bool@left'], outputs: ['q:bool@right'] };
  const good = { inputs: ['q:bool@left'], outputs: ['a:bool@right', 'b:bool@right'] };
  assert.equal(benchMismatch(good, subject), null);
  const wrongType = { inputs: ['q:int@left'], outputs: ['a:bool@right', 'b:bool@right'] };
  assert.match(benchMismatch(wrongType, subject), /bench inputs/);
  const missing = { inputs: ['q:bool@left'], outputs: ['a:bool@right'] };
  assert.match(benchMismatch(missing, subject), /bench outputs/);
  // A bench's own start and result pins sit on its top and bottom.
  const withControls = {
    inputs: ['q:bool@left', 'start:bool@top'],
    outputs: ['a:bool@right', 'b:bool@right', 'result:string@bottom'],
  };
  assert.equal(benchMismatch(withControls, subject), null);
});

test('finds listed admin gates by whole class name', () => {
  const text = '  %3 = Exec_PlayerState_GetFromEntity(Entity: %c)\n  %4 = Exec_PlayerState_GetDisplayName(PlayerState: %3)';
  assert.deepEqual(adminGatesIn(text, ['Exec_PlayerState_GetFromEntity', 'Exec_Controller_GetFromEntity']), [
    'Exec_PlayerState_GetFromEntity',
  ]);
  assert.deepEqual(adminGatesIn(text, ['Exec_PlayerState_GetDisplay']), []);
});

test('inline prefabs are found, nested ones too, ignoring strings and comments', () => {
  const code = [
    'let a = $```',
    '@left in x: bool',
    'let b = $```',
    'var n: int = 0',
    '```',
    '```',
    'let s = "$```not a prefab```"',
    '// $``` not one either',
  ].join('\n');
  const bodies = inlinePrefabs(code);
  assert.equal(bodies.length, 2);
  assert.ok(bodies[0].includes('@left in x: bool') && bodies[0].includes('var n: int = 0'));
  assert.equal(bodies[1].trim(), 'var n: int = 0');
  assert.deepEqual(inlinePrefabs('let t = "no prefab here"'), []);
});

test('asset references must be in the inventory; prefab paths are not assets', () => {
  const inventory = { assetTypes: [{ type: 'BRItemBase', assets: [{ name: 'Weapon_Bandage' }] }] };
  assert.deepEqual(unknownAssets('let a = $BRItemBase/Weapon_Bandage\nlet p = $./box.brz', inventory), []);
  assert.deepEqual(unknownAssets('let a = $BRItemBase/Weapon_Bandaid\nlet s = $Nope/X', inventory), [
    '$BRItemBase/Weapon_Bandaid',
    '$Nope/X',
  ]);
});
