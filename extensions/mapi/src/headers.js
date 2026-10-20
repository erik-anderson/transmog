import {header,latin1,notice} from './shared.js';

export function visualize(input){
  const message=input[input.direction];
  if(!message)return notice('MAPI/HTTP headers','unavailable','This message has no retained headers.');
  const operation=header(input.request,'x-requesttype');
  const roots=message.headers.map((field,index)=>({id:'h.'+index,name:field.name,
    value:{kind:'string',value:field.redacted?'Redacted':field.value?latin1(field.value).slice(0,16384):'Unavailable'},
    format:{protocol:'MAPI/HTTP header'},children:[]}));
  return {kind:'document',document:{title:'MAPI/HTTP headers',status:operation?'complete':'partial',presentation:'tree',
    summary:operation?'Operation: '+operation:'The request operation is unavailable or redacted.',roots}};
}
