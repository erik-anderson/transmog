export interface AuthSource { kind:string;label:string;occurrence:number;headerIndex?:number|null;offset:number|null;length:number|null;transforms:string[] }
export interface AuthField { id:string;name:string;value:string;sensitive:boolean;available:boolean;note:string;revealed?:boolean;busy?:boolean;error?:string;visibilityLabel?:string }
export interface AuthFinding { id:string;format:string;role:string;recognition:string;status:string;verification:string;source:AuthSource;fields:AuthField[];children:AuthFinding[];diagnostics:string[];readableFields?:AuthField[];technicalFields?:AuthField[] }
export interface AuthHandle { owner:string;slot:string;generation:string;pageId:string;revision:string }
export interface AuthSelection { sessionId:string;boundary:string;key:string }
export interface AuthView { handle:AuthHandle;inspection:{findings:AuthFinding[];coverage:Array<{source:string;status:string;detail:string}>;diagnostics:string[]} }
export interface AuthRelatedEntry { sessionId:string;boundary:string;label:string;reason:string }
export interface AuthRelated { entries:AuthRelatedEntry[];searched:number;limited:boolean;detail:string }
export interface AuthSourceNavigation { sessionId:string;boundary:string;source:AuthSource }
export const authBodyFormats:Record<string,{label:string;expected:string}>={
  jose:{label:'JWT / JWS / JWE',expected:'a single compact JWT, JWS or JWE token'},
  saml:{label:'SAML XML',expected:'a SAML XML document'},
  ntlm:{label:'NTLM',expected:'a single NTLM token, as Base64 text or binary'},
  negotiate:{label:'Negotiate',expected:'a single Negotiate token, as Base64 text or binary'},
};
