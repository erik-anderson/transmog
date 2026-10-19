import type {CapturedPageReport,ExperimentSelection} from './models.js';

export interface SavedDemoRevision {id:string;revision:number;}
export interface SavedDemoSource extends SavedDemoRevision {name:string;ownerId:string;}
export interface SavedDemo extends SavedDemoSource {
  ownerIncarnation:string;createdAt:number;modifiedAt:number;selection:ExperimentSelection;
  entryId:string;url:string;scope:'original-trace'|'all-loaded';available:number;skipped:number;bytes:number;
  availabilityError?:string|null;
}
export interface SavedDemoPreparation {token:string;name:string;replace:SavedDemoRevision|null;report:CapturedPageReport;}
export interface SavedDemoPreviewIntent {ownerId:string;demo:SavedDemo;isCurrent:()=>boolean;}
