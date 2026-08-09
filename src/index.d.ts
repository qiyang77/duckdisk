interface DiskItem {
  id: string;
  name: string;
  displayName?: string;
  cloudId?: string;
  value: number;
  size: number;
  allocatedSize?: number;
  isDirectory: boolean;
  children: Array<DiskItem>;
}

interface D3HierarchyDiskItemArc {
  x0: number;
  x1: number;
  y0: number;
  y1: number;
}
interface D3HierarchyDiskItem extends d3.HierarchyRectangularNode<DiskItem> {
  target: D3HierarchyDiskItemArc;
  current: D3HierarchyDiskItemArc;
  parent: any;
  children: this[];
  data: DiskItem;
  each: any;
}
