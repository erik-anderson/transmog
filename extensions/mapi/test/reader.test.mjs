import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BinaryReader } from '../dist/reader.js';
import { ParseBudget, ParseError } from '../dist/model.js';

test('little-endian scalars preserve signedness and all 64 bits', () => {
  const reader = new BinaryReader([255, 255, 255, 255, 1, 0, 0, 0, 0, 0, 32, 0], 'body');
  assert.equal(reader.integer('i32'), -1);
  assert.equal(reader.integer64(false), 9_007_199_254_740_993n);
  const negative = new BinaryReader(Array(8).fill(255), 'body');
  assert.equal(negative.integer64(true), -1n);
});

test('zero-sized and nested buffers cannot escape their parent', () => {
  const reader = new BinaryReader([1, 2, 3, 4], 'body');
  const empty = reader.child(0);
  assert.throws(() => empty.integer('u8'), ParseError);
  assert.equal(reader.offset, 0);
  const child = reader.child(2);
  assert.throws(() => child.child(3), ParseError);
  assert.deepEqual(child.take(2), [1, 2]);
  assert.deepEqual(reader.take(2), [3, 4]);
  for (const length of [-1, NaN, Infinity, 0.5]) {
    assert.throws(() => reader.child(length), ParseError);
  }
});

test('strings, CRLF and GUID byte order are explicit', () => {
  assert.equal(new BinaryReader([0x41, 0xff, 0], 'a').terminatedString(false), 'A?');
  assert.equal(new BinaryReader([0xa9, 0x03, 0, 0], 'w').terminatedString(true), 'Ω');
  assert.equal(new BinaryReader([68, 79, 78, 69, 13, 10], 'line').line(), 'DONE');
  const guid = new BinaryReader([0x33, 0x22, 0x11, 0, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff], 'guid');
  assert.equal(guid.guid(), '00112233-4455-6677-8899-aabbccddeeff');
  assert.throws(() => new BinaryReader([65], 'broken').terminatedString(true), ParseError);
  assert.throws(() => new BinaryReader([65], 'broken').utf16(1), ParseError);
});

test('IEEE-754 and reader work share a bounded budget', () => {
  assert.equal(new BinaryReader([0, 0, 128, 63], 'float').floatingPoint(4), 1);
  const reader = new BinaryReader([1, 2], 'body', new ParseBudget(10, 4, 2));
  const child = reader.child(1);
  assert.equal(child.integer('u8'), 1);
  assert.throws(() => reader.integer('u8'), (error) => error.kind === 'limit');
});
