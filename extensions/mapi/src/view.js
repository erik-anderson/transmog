import {base64} from './shared.js';

const MAX_NODES=18000,MAX_TEXT=2*1024*1024,MAX_DERIVED=2*1024*1024;
let epoch=0;

/** Host-owned tree/table/text/hex presentation; never markup or executable UI. */
export function view(decoded,origins,title,diagnostics=[],previous){
  const revision=++epoch,expanded=new Set(previous?.expanded??[]),identities=new WeakMap(),known=new Map(origins),sources=[];
  let next=0,derivedBytes=0;
  const visit=block=>{if(!block)return;identities.set(block,'n.'+next++);for(const child of block.children)visit(child);};
  visit(decoded.tree);
  let sourceLimited=false;
  for(const source of decoded.derived){
    derivedBytes+=source.bytes.length;
    if(derivedBytes>MAX_DERIVED){sourceLimited=true;continue;}
    known.set(source.id,source.bytes.length);
    sources.push({id:source.id,name:'Decoded MAPI buffer '+(sources.length+1),bytesBase64:base64(source.bytes),parents:source.parents,transform:source.transform,exactMapping:source.exactMapping});
  }
  function document(){
    const tokens=new Map();let count=0,text=0,limited=sourceLimited||diagnostics.some(item=>item.code==='input-limit'),malformed=false,partial=false;
    const bounded=(value,max)=>{value=String(value);const room=Math.max(0,Math.min(max,Math.floor((MAX_TEXT-text)/3)));if(value.length>room)limited=true;const result=value.slice(0,room);text+=result.length*3;return result;};
    function node(block,depth){
      count++;
      const id=identities.get(block),type=block.type.split('.').at(-1),label=block.text??type;
      const scalar=block.raw!==null&&block.raw!==undefined;
      const name=scalar||block.type==='BlockParser.BlockBytes'||block.stringValue!==undefined||!block.children.length?(label.includes(':')?label.split(':',1)[0]:type):label;
      const result={id,name:bounded(name,384),children:[],format:{protocol:bounded(type,128),byteOrder:'little-endian'}};
      if(block.type==='BlockParser.BlockException'){
        if(label.includes('missing information')||label.includes('Failed locating'))partial=true;else malformed=true;
      }
      if(!block.parsed||label==='Remaining Data'||label==='Unparsed data')partial=true;
      if(block.stringValue!==undefined){result.value={kind:'string',value:bounded(block.stringValue,16384)};result.format.charset=block.charset;result.presentation='text';}
      else if(block.timestamp){result.value={kind:'timestamp',value:block.timestamp};result.format.units='UTC';}
      else if(block.flags){result.value={kind:'flags',value:{names:bounded(block.enumName,256).split(', ').slice(0,64),raw:block.raw}};result.format.radix=16;}
      else if(block.enumName!==undefined){result.value={kind:'enum',value:{name:bounded(block.enumName,256),raw:block.raw}};result.format.radix=16;}
      else if(scalar){
        if(block.valueType==='System.Boolean')result.value={kind:'boolean',value:block.raw==='True'};
        else if(block.valueType==='System.Guid')result.value={kind:'guid',value:block.raw};
        else if(block.valueType==='System.Single'||block.valueType==='System.Double'){
          const value=Number(block.raw);result.value=Number.isFinite(value)?{kind:'float',value}:{kind:'string',value:block.raw};result.format.encoding='IEEE 754';
        }else if(block.valueType==='System.Char')result.value={kind:'string',value:block.raw};
        else result.value={kind:'integer',value:block.raw};
      }else if(block.type==='BlockParser.BlockBytes'){result.name=bounded(label.split(':',1)[0],256);result.value={kind:'bytes'};result.presentation='hex';}
      else if(block.children.length){result.value={kind:'object'};}
      else if(label)result.value={kind:'string',value:bounded(label,16384)};
      if(type==='PtypCurrency')result.format.units='1/10000 currency unit';
      if(type==='PtypTime'&&!block.timestamp)result.format.units='100 ns since 1601-01-01 UTC';
      const length=known.get(block.sourceId),offset=BigInt(block.offset),size=BigInt(block.size);
      if(length!==undefined&&size>0n&&offset>=0n&&offset+size<=BigInt(length))result.spans=[{sourceId:block.sourceId,offset:block.offset,length:block.size}];
      if(block.children.length){
        const token='e.'+revision+'.'+id;
        if(depth>=31){limited=true;result.presentation='hex';}
        else if((depth<6&&block.children.length<=6)||expanded.has(id)){
          for(const child of block.children){if(count>=MAX_NODES){limited=true;break;}result.children.push(node(child,depth+1));}
          if(result.children.length<block.children.length)result.name=bounded(result.name+' (display limit)',384);
        }else {result.childrenToken=token;tokens.set(token,block);}
        if(block.children.length>1&&block.children.every(child=>child.type===block.children[0].type)&&block.children[0].type.includes('PropertyRow'))result.presentation='table';
      }
      if(block.timestamp){
        if(count<MAX_NODES&&depth<32){count++;result.children.push({id:id+'.ticks',name:'FILETIME ticks',value:{kind:'integer',value:block.raw},format:{units:'100 ns since 1601-01-01 UTC',byteOrder:'little-endian'},spans:result.spans??[]});}
        else limited=true;
      }
      return result;
    }
    // Diagnose the entire parsed structure even when branches are deferred.
    const inspect=block=>{if(!block)return;if(block.type==='BlockParser.BlockException'){if((block.text??'').includes('Failed locating'))partial=true;else malformed=true;}if(!block.parsed||block.text==='Remaining Data'||block.text==='Unparsed data')partial=true;for(const child of block.children)inspect(child);};
    inspect(decoded.tree);
    const roots=decoded.tree?[node(decoded.tree,0)]:[];
    const extra=limited?[{code:'display-limit',message:'Some display values or decoded byte sources exceed the visualizer limit. Original retained bytes remain available in Hex.'}]:[];
    const status=limited?'limited':malformed?'malformed':partial||diagnostics.length?'partial':decoded.tree?'complete':'unavailable';
    return {step:{kind:'document',document:{title,status,presentation:'tree',roots,sources,diagnostics:[...diagnostics,...extra]}},tokens};
  }
  let current=document();
  return {expanded,get step(){return current.step;},expand(token){const block=current.tokens.get(token);if(!block)throw new Error('Expired MAPI expansion');expanded.add(identities.get(block));current=document();return current.step;}};
}
