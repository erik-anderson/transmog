// Developer-only VM harness; production isolation is the native AppContainer worker.
import vm from 'node:vm';
import {readFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {createInterface} from 'node:readline';

const context=vm.createContext({}, {codeGeneration:{strings:false,wasm:false}}),modules=new Map();
vm.runInContext('globalThis.Function=undefined;globalThis.eval=undefined;',context);
const root=new URL('../',import.meta.url);
async function module(url){
  if(!url.href.startsWith(root.href)||!url.pathname.endsWith('.js'))throw new Error('Fixture module escaped the extension');
  if(modules.has(url.href))return modules.get(url.href);
  const item=new vm.SourceTextModule(await readFile(fileURLToPath(url),'utf8'),{context,identifier:url.href});
  modules.set(url.href,item);return item;
}
const entry=await module(new URL('../src/protocol.js',import.meta.url));
await entry.link((specifier,owner)=>module(new URL(specifier,owner.identifier)));
await entry.evaluate({timeout:15000});
for await(const line of createInterface({input:process.stdin,crlfDelay:Infinity})){
  try{
    const fixture=JSON.parse(line),messages=[...(fixture.context??[]),fixture.message].map((message,index)=>{
      const id=message.order||index+1;
      return {id,requestType:message.requestType,...Object.fromEntries(['request','response'].map(side=>[side,{bytes:Array.from(Buffer.from(message[side]??'','base64')),sourceId:`body.${id}.${side}`}]))};
    });
    context.fixtureMessages=messages;context.fixtureSelected=messages.at(-1).id;context.fixtureDirection=fixture.message.direction;
    context.fixtureOptions={type:fixture.type,arguments:fixture.arguments,offset:fixture.offset,count:fixture.count,prepareRequest:fixture.prepareRequest};context.decode=entry.namespace.decode;
    const outcome=vm.runInContext('decode(fixtureMessages,fixtureSelected,fixtureDirection,fixtureOptions)',context,{timeout:10000});
    console.log(JSON.stringify(fixture.includeDerived?outcome:outcome.tree));
  }catch(error){console.log(JSON.stringify({failure:error.name,message:error.message}));}
  finally{delete context.fixtureMessages;delete context.fixtureOptions;delete context.decode;}
}
