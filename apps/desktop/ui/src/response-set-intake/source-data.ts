import { invoke } from "@tauri-apps/api/core";
import type { AutomationStatus, ResponseAsset, ResponseSetIntakeRequest } from "../models.js";

interface TrafficRow {
  id: string;
  inPage: boolean;
  requestNumber: string | null;
  method: string | null;
  url: string;
  status: number | null;
  retainedBytes: number;
  unavailableReason: string | null;
}

export interface IntakeTrafficPage {
  rows: TrafficRow[];
  nextOffset: number | null;
}

export interface IntakeData {
  status: AutomationStatus;
  assets: ResponseAsset[];
  traffic: IntakeTrafficPage | null;
}

export function queryIntakeTraffic(
  offset: number,
  search: string,
  selectedIds: string[],
): Promise<IntakeTrafficPage> {
  return invoke("query_response_set_intake_traffic", {
    input: { offset, search, selectedIds: selectedIds.slice(0, 1_000) },
  });
}

export async function loadIntakeData(request: ResponseSetIntakeRequest): Promise<IntakeData> {
  const [status, assets, traffic] = await Promise.all([
    invoke<AutomationStatus>("automation_status"),
    request.source === "rules" ? invoke<ResponseAsset[]>("response_assets") : [],
    request.source === "traffic" ? queryIntakeTraffic(0, "", request.ids ?? []) : null,
  ]);
  return { status, assets, traffic };
}
