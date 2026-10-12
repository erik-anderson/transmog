import {zstdCompressSync} from 'node:zlib';
import {createHash} from 'node:crypto';

// Deterministic encoder for the plain packed-object TMCap envelope used by native
// probes. IDs and offsets in these fixtures stay within exact JS integers.
export function nativeFixture(records, exchangeId) {
  const sort=value=>Array.isArray(value)?value.map(sort):value&&typeof value==='object'?Object.fromEntries(Object.keys(value).sort().map(key=>[key,sort(value[key])])):value;
  const expanded=[];
  const known=new Set();
  for(const original of structuredClone(records)) {
    if(original.kind==='body-segment'&&Array.isArray(original.payload.bytes)&&original.payload.bytes.length>128*1024) {
      const bytes=original.payload.bytes;
      for(let offset=0;offset<bytes.length;offset+=128*1024) {
        const part=bytes.slice(offset,offset+128*1024),last=offset+part.length===bytes.length;
        expanded.push({...original,payload:{...original.payload,bytes:part,byte_count:part.length+(last?Math.max(0,original.payload.byte_count-bytes.length):0)}});
      }
      continue;
    }
    if(original.kind==='performance') {
      original.payload.work??=[];
      original.payload.transports=original.payload.transports.map(transport=>{
        const context=Object.fromEntries(['tunnelDestination','outcome','leg','connectionId','peer','local','tlsVersion','tlsResumed','tls','cipher','alpn'].map(key=>[key,transport[key]??null]));
        context.outcome??='connected';
        const sha256=createHash('sha256').update(JSON.stringify(sort(context))).digest('hex');
        if(!known.has(sha256)){known.add(sha256);expanded.push({kind:'connection-context',exchange_id:0,sequence:0,payload:{sha256,context}});}
        return {context:sha256,sampledOffsetMicros:transport.sampledOffsetMicros??0,shared:transport.shared??false,setupTimings:transport.setupTimings??[],...Object.fromEntries(['dnsMicros','tcpMicros','tlsMicros','bytesRead','bytesWritten','tcp','tcpSampledOffsetMicros','socketIo','quic'].map(key=>[key,transport[key]??null]))};
      });
    }
    expanded.push(original);
  }
  const blockSize=16384,payloadSize=blockSize-5,blockBytes=blockSize+16;
  const header=Buffer.from(JSON.stringify({version:1,layout:'indexed-blocks',block_size:blockSize,compression:'zstd',cipher:'none',kdf:'none',salt:Array(16).fill(0),nonce_prefix:Array(4).fill(0),memory_kib:0,iterations:0,lanes:0}));
  const size=Buffer.alloc(4);size.writeUInt32LE(header.length);
  const geometry=Buffer.alloc(4);geometry.writeUInt32LE(blockSize);
  const envelope=Buffer.concat([Buffer.from('TMCAP001\0'),size,header,geometry,Buffer.alloc(512)]);
  const pack=(kind,bytes)=>{
    const compressed=zstdCompressSync(bytes),useCompressed=compressed.length<bytes.length;
    const encoded=useCompressed?compressed:bytes;
    const prefix=Buffer.alloc(10);prefix[0]=kind;prefix[1]=Number(useCompressed);prefix.writeUInt32LE(bytes.length,2);prefix.writeUInt32LE(encoded.length,6);
    return Buffer.concat([prefix,encoded]);
  };
  const chunks=[],entries=[];let logicalBytes=0;
  const append=bytes=>{
    const offset=envelope.length+Math.floor(logicalBytes/payloadSize)*blockBytes+5+logicalBytes%payloadSize;
    chunks.push(bytes);logicalBytes+=bytes.length;return offset;
  };
  for(const [index,original] of [...expanded,{kind:'seal',sequence:0,exchange_id:0,payload:{record_count:expanded.length}}].entries()) {
    const record={revision:1,sequence:index+1,exchange_id:exchangeId,...structuredClone(original)};
    let body;
    if(record.kind==='body-segment'&&Array.isArray(record.payload.bytes)) {
      const bytes=Buffer.from(record.payload.bytes),object=pack(0,bytes);
      body={offset:append(object)};
      record.body_index={frame_bytes:object.length,bytes:bytes.length,digest:Array.from(createHash('sha256').update(bytes).digest())};record.payload.bytes=null;
    }
    entries.push({index,stored:record,...(body?{body}:{})});
  }
  const blocks=[];
  const put=(kind,payload)=>{
    const plain=Buffer.alloc(blockSize);plain[0]=kind;plain.writeUInt32LE(payload.length,1);payload.copy(plain,5);
    const ordinal=Buffer.alloc(8);ordinal.writeBigUInt64LE(BigInt(blocks.length));
    const checksum=createHash('sha256').update(header).update(ordinal).update('TMCap block').update(plain).digest().subarray(0,16);
    blocks.push(Buffer.concat([plain,checksum]));
  };
  const putObject=(kind,bytes)=>{for(let offset=0;offset<bytes.length;offset+=payloadSize)put(kind,bytes.subarray(offset,offset+payloadSize));};
  const packed=value=>{const bytes=Buffer.from(JSON.stringify(value)),prefix=Buffer.alloc(5);prefix.writeUInt32LE(bytes.length,1);return Buffer.concat([prefix,bytes]);};
  const index=pack(1,Buffer.from(JSON.stringify(entries))),offset=append(index);
  putObject(0,Buffer.concat(chunks));
  const root={offset,bytes:index.length,first:0,count:entries.length,level:0};
  const footer=packed({roots:[root],count:entries.length,sealed:true,block:blocks.length});
  if(footer.length>payloadSize)throw new Error('Fixture footer exceeds one block');
  put(3,footer);
  return Buffer.concat([envelope,...blocks]);
}
