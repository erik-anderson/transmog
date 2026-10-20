// Read-only Fiddler data adapters used by the parser, never a Fiddler host or capture API.
using System;
using System.Collections.Generic;

namespace Fiddler
{
    public sealed class HTTPHeaders
    {
        public string RequestPath { get; set; } = "/";
        private readonly Dictionary<string, string> headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        public string this[string name]
        {
            get => headers.TryGetValue(name, out var value) ? value : "";
            set => headers[name] = value;
        }
        public bool Exists(string name) => headers.ContainsKey(name);
        public bool ExistsAndContains(string name, string value) => Exists(name) && this[name].IndexOf(value, StringComparison.OrdinalIgnoreCase) >= 0;
    }

    public sealed class Session
    {
        public int id { get; set; }
        // Native scope checks have already selected the conversation; this is an inert
        // grouping key, not a grant to inspect a process or enumerate its sessions.
        public string LocalProcess { get; set; } = "";
        public HTTPHeaders RequestHeaders { get; set; } = new HTTPHeaders();
        public HTTPHeaders ResponseHeaders { get; set; } = new HTTPHeaders();
        public byte[] requestBodyBytes { get; set; } = Array.Empty<byte>();
        public byte[] responseBodyBytes { get; set; } = Array.Empty<byte>();
        public string this[string name]
        {
            get => name == "VirtualID" ? id.ToString() : name == "LocalProcess" ? LocalProcess : ResponseHeaders[name];
            set => ResponseHeaders[name] = value;
        }
    }
}
