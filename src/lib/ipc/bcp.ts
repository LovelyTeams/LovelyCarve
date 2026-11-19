import { invokeCmd } from './client';
import type { ImageId } from './ids';

export interface BcpField {
  key: string;
  name: string;
  label: string;
}

export interface BcpFile {
  name: string;
  rows: number;
}

export interface BcpDataset {
  code: string;
  encoding: string;
  fields: BcpField[];
  files: BcpFile[];
  rows: number;
}

export interface BcpCatalog {
  path: string;
  datasets: BcpDataset[];
  totalRows: number;
  totalFiles: number;
}

export interface BcpCursor {
  fileIndex: number;
  byteOffset: number;
  rowIndex: number;
}

export interface BcpRow {
  number: number;
  fileName: string;
  values: string[];
}

export interface BcpPage {
  rows: BcpRow[];
  nextCursor: BcpCursor | null;
}

export function bcpCatalog(imageId: ImageId): Promise<BcpCatalog> {
  return invokeCmd('bcp_catalog', { imageId });
}

export function bcpRows(imageId: ImageId, code: string, cursor: BcpCursor | null): Promise<BcpPage> {
  return invokeCmd('bcp_rows', { imageId, code, cursor });
}
