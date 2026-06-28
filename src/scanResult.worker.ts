const mapScanTree = (obj: any, parent: any = null): DiskItem => {
  if (obj.name === "(total)") {
    obj.id = "/";
    obj.name = "/";
  } else if (parent && parent.id === "/") {
    obj.id = obj.name;
    obj.name = obj.name.substring(1);
  } else {
    obj.id = parent ? parent.id + "/" + obj.name : obj.name;
  }

  if (obj.children && obj.children.length > 0) {
    obj.isDirectory = true;
    obj.value = obj.size;
    obj.children.forEach((child: DiskItem) => mapScanTree(child, obj));
  }

  return obj;
};

self.onmessage = (event: MessageEvent<string>) => {
  try {
    const parsed = JSON.parse(event.data);
    self.postMessage({ type: "done", tree: mapScanTree(parsed.tree) });
  } catch (error) {
    self.postMessage({
      type: "error",
      message: error instanceof Error ? error.message : String(error),
    });
  }
};

export {};
