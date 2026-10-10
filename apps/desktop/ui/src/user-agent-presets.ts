import groups from './user-agent-presets.json';
interface Metadata { brands:Array<{brand:string;version:string}>; fullVersion:string; platform:string; platformVersion:string; architecture:string; model:string; mobile:boolean; bitness?:string; wow64?:boolean; }
export interface UserAgentPreset {id:string;group:string;title:string;value:string;metadata:Metadata|null;}
export const userAgentPresets:UserAgentPreset[]=groups.flatMap((group,groupIndex)=>group.values.map((value,index)=>({...value,group:group.title,id:`preset-${groupIndex}-${index}`})));
const quote=(value:string):string=>JSON.stringify(value);
export function presetClientHints(preset:UserAgentPreset):Record<string,string> {
  const data=preset.metadata;if(!data)return {};
  const version=(value:string,full=false)=>value==='%s'?(full?'%full%':'%major%'):full&&!value.includes('.')?value+'.0.0.0':value;
  const brands=(full:boolean)=>data.brands.map(brand=>quote(brand.brand)+';v='+quote(version(brand.version,full))).join(', ');
  const hints:Record<string,string>={'Sec-CH-UA':brands(false),'Sec-CH-UA-Full-Version-List':brands(true),'Sec-CH-UA-Full-Version':quote(version(data.fullVersion,true)),'Sec-CH-UA-Platform':quote(data.platform),'Sec-CH-UA-Platform-Version':quote(data.platformVersion),'Sec-CH-UA-Arch':quote(data.architecture),'Sec-CH-UA-Model':quote(data.model),'Sec-CH-UA-Mobile':data.mobile?'?1':'?0'};
  if(data.bitness!==undefined)hints['Sec-CH-UA-Bitness']=quote(data.bitness);
  if(data.wow64!==undefined)hints['Sec-CH-UA-WoW64']=data.wow64?'?1':'?0';
  return hints;
}
