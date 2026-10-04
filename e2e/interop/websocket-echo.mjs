import { createHash } from 'node:crypto';
import { createServer } from 'node:net';

const maximumFrameBytes = 1024 * 1024;
const server = createServer(socket => {
  let upgraded = false;
  let buffer = Buffer.alloc(0);
  socket.on('data', chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    if (!upgraded) {
      const boundary = buffer.indexOf('\r\n\r\n');
      if (boundary < 0) return;
      const head = buffer.subarray(0, boundary).toString('latin1');
      buffer = buffer.subarray(boundary + 4);
      const key = header(head, 'sec-websocket-key');
      if (!/^GET \/socket HTTP\/1\.1$/m.test(head) || !key) {
        socket.end('HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n');
        return;
      }
      const accept = createHash('sha1')
        .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
        .digest('base64');
      socket.write(
        'HTTP/1.1 101 Switching Protocols\r\n'
          + 'Connection: Upgrade\r\n'
          + 'Upgrade: websocket\r\n'
          + `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
      );
      upgraded = true;
    }
    while (upgraded) {
      const frame = readFrame(buffer);
      if (!frame) return;
      buffer = buffer.subarray(frame.consumed);
      if (frame.opcode === 0x1) {
        const text = frame.payload.toString('utf8');
        console.log(`ECHO=${text}`);
        socket.write(encodeFrame(0x1, frame.payload));
      } else if (frame.opcode === 0x9) {
        socket.write(encodeFrame(0xa, frame.payload));
      } else if (frame.opcode === 0x8) {
        socket.end(encodeFrame(0x8, frame.payload));
        return;
      }
    }
  });
});

server.listen(0, '127.0.0.1', () => {
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('unexpected listen address');
  console.log(`LISTEN_ADDR=127.0.0.1:${address.port}`);
});

function header(head, name) {
  const line = head.split('\r\n').find(value => value.toLowerCase().startsWith(`${name}:`));
  return line?.slice(line.indexOf(':') + 1).trim();
}

function readFrame(bytes) {
  if (bytes.length < 2) return undefined;
  const opcode = bytes[0] & 0x0f;
  const masked = (bytes[1] & 0x80) !== 0;
  let length = bytes[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (bytes.length < 4) return undefined;
    length = bytes.readUInt16BE(2);
    offset = 4;
  } else if (length === 127) {
    if (bytes.length < 10) return undefined;
    const wide = bytes.readBigUInt64BE(2);
    if (wide > BigInt(maximumFrameBytes)) throw new Error('WebSocket frame is too large');
    length = Number(wide);
    offset = 10;
  }
  if (!masked || length > maximumFrameBytes) throw new Error('invalid client WebSocket frame');
  if (bytes.length < offset + 4 + length) return undefined;
  const mask = bytes.subarray(offset, offset + 4);
  offset += 4;
  const payload = Buffer.from(bytes.subarray(offset, offset + length));
  for (let index = 0; index < payload.length; index++) payload[index] ^= mask[index % 4];
  return { opcode, payload, consumed: offset + length };
}

function encodeFrame(opcode, payload) {
  if (payload.length < 126) return Buffer.concat([Buffer.from([0x80 | opcode, payload.length]), payload]);
  const head = Buffer.alloc(4);
  head[0] = 0x80 | opcode;
  head[1] = 126;
  head.writeUInt16BE(payload.length, 2);
  return Buffer.concat([head, payload]);
}
