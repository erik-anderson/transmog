export function visualize(input) {
  const status = input.response?.status;
  return {
    kind: "document",
    document: {
      title: "Response overview",
      status: status == null ? "unavailable" : "complete",
      presentation: "tree",
      roots: [
        {
          id: "http-status",
          name: "HTTP status",
          value: {
            kind: "string",
            value: status == null ? "No retained response status" : String(status),
          },
        },
      ],
    },
  };
}
