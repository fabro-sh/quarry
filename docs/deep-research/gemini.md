# **Architectural Blueprint and Strategic Implementation Plan for Quarry: A Unified Collaborative Storage and Agent Memory Substrate**

## **1\. Executive Overview and Architectural Vision**

The rapid proliferation of autonomous artificial intelligence (AI) agents has fundamentally altered the landscape of human-computer interaction. Historically, software environments were strictly compartmentalized: humans utilized graphical user interfaces (GUIs) and rich-text editors to draft documents, while programmatic processes interacted with databases and file systems via highly structured application programming interfaces (APIs). The introduction of multi-agent systems—where humans and AI agents act as co-authors, reviewers, and iterative developers—necessitates a unified data storage infrastructure that transcends these traditional boundaries. "Quarry" is conceptualized to be this exact infrastructure.  
Quarry is a next-generation collaborative data storage layer engineered to fuse the decentralized version control capabilities of Git, the real-time conflict-free collaboration of modern web editors, the native operating system accessibility of virtual file systems, and the long-horizon cognitive retention of advanced AI memory architectures. The primary objective of the Quarry architecture is to provide a cohesive substrate where human users and AI agents can simultaneously co-author, query, and manipulate data without encountering state corruption, merge conflicts, or interface friction.  
To achieve this ambitious synthesis, the Quarry architecture is designed across multiple intersecting planes. At its foundational core, Quarry leverages a Conflict-Free Replicated Data Type (CRDT) engine to manage application state across distributed nodes instantaneously.1 This real-time state is continuously and bi-directionally synchronized with a standard Git repository to maintain legacy ecosystem compatibility, branch management, and historical provenance.3 Above this CRDT layer, the system utilizes the Filesystem in Userspace (FUSE) protocol—alongside Server Message Block (SAMBA) mapping—to project the collaborative data as a standard POSIX-compliant virtual file system. This allows human operators and AI agents to interact with the repository using native command-line utilities, such as grep and ripgrep, as if the data were stored on a traditional hard drive.5  
Simultaneously, Quarry acts as the underlying database for advanced cognitive memory architectures. By supporting systems inspired by TrueMemory, AgentMemory, and Supermemory, Quarry enables AI agents to encode, deduplicate, and retrieve long-horizon contextual data continuously as they interact with the file system.8 At the presentation layer, a user-facing interface facilitates real-time collaborative drafting, redlining, and provenance tracking. Powered by PlateJS and a headless synchronization bridge akin to Every's Proof SDK, this interface allows humans to leave targeted critiques for agents on temporary .draft files, merging them seamlessly into the permanent record upon approval.10 Furthermore, Quarry extends its collaborative capabilities to binary formats, enabling immutable PDF storage coupled with CRDT-driven metadata annotations.13  
This comprehensive research report provides an exhaustive architectural blueprint for Quarry. It dissects each technical subsystem, evaluates implementation paradigms, contrasts competing technological standards, and establishes a cohesive, phased deployment strategy for constructing this unified collaborative storage layer.

## **2\. The Algorithmic Foundation: Conflict-Free Replicated Data Types (CRDTs)**

The core challenge of any collaborative system is maintaining a synchronized state across distributed actors who may read and write data concurrently. Traditional distributed databases often rely on strongly consistent replication, utilizing distributed locking mechanisms or consensus algorithms that severely degrade performance and violate local-first principles.2 Version control systems like Git rely on asynchronous branching and merging, which inevitably leads to manual conflict resolution when multiple actors modify adjacent lines of text.15 Quarry circumvents these limitations by utilizing a Conflict-Free Replicated Data Type (CRDT) as its foundational data engine.

### **2.1 The Mathematics and Mechanics of CRDTs**

A CRDT is a specialized data structure designed to be replicated across multiple computers in a network, allowing any replica to be updated independently, concurrently, and without coordination with other replicas.16 When an algorithm forms the basis of a CRDT, it automatically resolves any inconsistencies that might occur, guaranteeing that although replicas may possess different states at any particular microsecond, they will eventually converge to identical states once all operations are transmitted.1  
All operations within a state-based or operation-based CRDT must satisfy three strict mathematical properties to ensure strong eventual consistency. First, the operations must be associative, meaning the order of merging distinct branches does not alter the final outcome.1 Second, the operations must be commutative, ensuring that the identity of the replica merging the data is irrelevant to the final state.1 Finally, the operations must be idempotent, meaning that if a network error causes the same state change to be merged multiple times, the data structure does not suffer from duplication anomalies.1 This mathematical foundation is what allows Quarry to accept concurrent modifications from a human typing in a web browser and an AI agent executing a massive search-and-replace operation via the command line, resolving the inputs seamlessly.

### **2.2 Evaluating CRDT Implementations for Quarry**

The open-source ecosystem provides several general-purpose CRDT libraries, each optimized for different use cases and data models.17 The selection of the underlying CRDT engine is the most consequential architectural decision for Quarry.

| CRDT Framework | Primary Optimization | Key Advantages | Notable Limitations | Reference |
| :---- | :---- | :---- | :---- | :---- |
| **Yjs** | Rich-text and generic shared types (Maps, Arrays). | Unmatched performance, native bindings for rich-text editors, modular network and persistence layers, highly compressed binary updates. | State history can grow over time without careful garbage collection management. | 18 |
| **Automerge** | JSON data models and local-first software. | Excellent for structured document data, preserves all user input histories, strong theoretical backing. | Resolves concurrent edits to the same JSON key arbitrarily; historically slower than Yjs for massive text operations. | 17 |
| **Loro** | Moveable trees and Peritext-like rich text. | Integrates the Fugue algorithm to minimize interleaving anomalies, natively supports directory-like data manipulation. | Newer ecosystem, larger WASM binary size compared to native JavaScript implementations. | 22 |
| **Diamond Types** | Plain text editing. | Extremely fast for plain text operations. | Lacks native support for complex nested JSON structures or rich-text formatting required by web editors. | 17 |

For the Quarry architecture, Yjs emerges as the optimal foundation. Yjs exposes its internal data structures as shared types (e.g., Y.Map, Y.Array, Y.Text) which act like standard data types but distribute changes automatically to peers without merge conflicts.18 Yjs is fundamentally network-agnostic, allowing Quarry to route synchronization data over WebSockets for web clients, and over internal memory channels for local file system operations.18 Furthermore, changes on a shared Yjs document are encoded into highly compressed binary updates, making the storage footprint incredibly efficient compared to JSON-based operation logs.23

### **2.3 The CRDT-over-FS Architecture**

While Yjs easily binds to web editors, bridging the gap between a CRDT engine and a local file system requires a specialized structural approach. Standard file systems do not natively understand CRDT state vectors. To resolve this, Quarry implements a "CRDT-over-FS" architecture, leveraging the local file system purely as an append-only application storage layer.24  
This architecture draws inspiration from the BitCask database model.25 Instead of updating traditional files in place—which would corrupt CRDT state if an agent and a human saved concurrently—Quarry serializes all writes into append-only session log files.25 Every active process interacting with Quarry, whether it is the FUSE daemon, a background Python script, or the headless synchronization server, is assigned a unique Process ID (Pid).25  
Each process writes to its own dedicated, locked subdirectory.25 When a process commits a change, it appends a serialized entry frame to its active session log. This frame contains the precise length of the entry, a hybrid logical clock timestamp, the operation key, the operation payload (a Yjs Uint8Array binary update), and a CRC32 checksum to guarantee disk-level integrity.23 A separate .loglist file tracks the lifecycle of these session logs, marking them with sizes when they are finalized.25  
When the Quarry daemon needs to project the current state of a file, it does not read a single plain-text file. Instead, an in-memory DashMap index rapidly reads the session logs from all process subdirectories, merging the Yjs binary updates chronologically.25 By utilizing this CRDT-over-FS approach, Quarry entirely eliminates the need for blocking file locks. An AI agent can stream gigabytes of generated code into its session log simultaneously while a human user streams keystrokes into theirs; the CRDT engine merges these isolated append-only logs in memory to present a unified, conflict-free state.25

## **3\. Native File System Projection: FUSE and SAMBA**

For Quarry to function as a true substrate for AI agents, it cannot exist solely as a web application or a proprietary API. Modern AI coding agents heavily rely on native operating system tools—ranging from simple ls and cat commands to complex grep, ripgrep, and language server protocol (LSP) indexing.5 To accommodate this, Quarry must project its internal CRDT state into the operating system as a standard, POSIX-compliant file system.

### **3.1 The FUSE (Filesystem in Userspace) Implementation**

The primary mechanism for this projection is FUSE. The FUSE framework consists of a kernel module (fuse.ko), a userspace library (libfuse), and a mount utility (fusermount).5 When an AI agent executes a command like ripgrep across a Quarry directory, the Virtual File System (VFS) layer in the operating system kernel intercepts the system call and routes it to the FUSE kernel module.5 The kernel module then forwards the request back into userspace, specifically to the Quarry daemon.5  
To implement this reliably across platforms, Quarry utilizes Go-based FUSE wrappers, specifically cgofuse, which provides robust cross-platform compatibility, including FUSE3 support on Linux and FreeBSD, and FUSE-T support on macOS.26 The Quarry daemon must implement several critical interfaces to translate POSIX commands into CRDT operations. When an agent issues a read command, the FUSE Read function intercepts the request.6 The Quarry daemon queries the CRDT-over-FS index, computes the latest plain-text state of the requested Y.Text document, and streams the result back to the kernel buffer.6  
Conversely, when an agent writes to a file, the FUSE Write and Flush operations are triggered.6 Because the agent is simply writing plain text blocks, the Quarry FUSE daemon must calculate the textual delta between the previous CRDT state and the agent's new buffer. It then encodes this delta as a Yjs update and appends it to the process's session log, effectively translating a dumb POSIX write into a mathematically secure CRDT operation.23

### **3.2 Network Mapping via SAMBA**

While FUSE is ideal for local processes, multi-agent systems frequently execute within isolated Docker containers, Kubernetes pods, or remote virtual machines where injecting FUSE kernel capabilities (--cap-add SYS\_ADMIN, \--device /dev/fuse) is prohibited by strict security policies. To ensure Quarry remains universally accessible, the architecture incorporates native SAMBA (SMB/CIFS) network mapping.  
By running an embedded Samba daemon or utilizing a userspace SMB server protocol, Quarry can broadcast its directory structure over the local network. Remote agents can mount the Quarry repository using standard network file system protocols. The internal architecture remains identical; incoming SMB read and write requests are routed through the same Yjs delta-calculation engine as the FUSE requests. This dual-protocol approach guarantees that Quarry can ingest agent modifications regardless of the agent's host environment constraints.

### **3.3 Access Control, Masks, and the Deletion Problem**

In a hybrid environment where autonomous agents and human users share a file system, access control is a critical security vector. A malfunctioning or hallucinating AI agent possessing full read-write access could theoretically execute a recursive deletion command, wiping out critical project infrastructure. Quarry mitigates this risk by dynamically enforcing file permissions at the FUSE and SAMBA translation layer.  
The FUSE implementation utilizes the Access interface, which receives the file path and a permission mask.6 Quarry implements an intelligent routing mechanism tied to the file's metadata status. If a document is flagged as "published" or "human-approved," the Quarry daemon intercepts the fuse.DELETE\_OK bitwise mask.27 If an agent attempts to delete or overwrite the file, the FUSE daemon returns a \-fuse.EPERM (Operation not permitted) error to the kernel.27  
Furthermore, Quarry utilizes Chmod3 and Chown3 interfaces to project files with explicit read-only (ro) or read-write (rw) flags depending on the identity of the process accessing them.26 Agents are restricted to writing only within designated .draft files or specific staging directories, guaranteeing that core system files remain immutable to autonomous processes.

### **3.4 Caching Topologies and Hit/Miss Analytics**

A known limitation of FUSE is the latency introduced by constant context switching between the kernel and userspace, which can bottleneck performance during massive read-heavy operations like an agent indexing a repository.28 To overcome this, Quarry implements a sophisticated multi-layered caching topology modeled on enterprise systems like Cloud Storage FUSE and DFUSE.28

| Cache Feature | Mechanism | Benefit for Quarry Architecture | Reference |
| :---- | :---- | :---- | :---- |
| **Userspace Read Cache** | Stores copies of frequently accessed, fully resolved CRDT plain-text states locally in high-speed media. | Serves repeat reads instantly without forcing the CRDT engine to recompute the document state from the append-only session logs. | 31 |
| **Write-Back Kernel Cache** | Offloads userspace consistency control to the kernel driver, deferring immediate synchronization. | Achieves higher throughput for burst-write operations (e.g., an agent dumping a large log file) by preventing immediate CRDT serialization overhead. | 28 |
| **Parallel Prefetching** | Utilizes multiple background workers to resolve CRDT states into buffers proactively. | Accelerates the loading of large binary assets or AI model checkpoints by using the file cache directory as a prefetch buffer. | 31 |

To monitor the efficiency of this caching topology, Quarry projects dynamic hit/miss statistics directly into the virtual file system. Inspired by netstatfs, which maps network statistics to directories 7, Quarry maintains a .quarry/.stats virtual directory.32 Agents and system administrators can simply cat.quarry/.stats/cache\_hits.txt to read real-time telemetry. If the cache miss rate climbs too high, the Quarry daemon can dynamically increase the RAM allocation for the in-memory DashMap index to improve read speeds.25

### **3.5 Ephemeral Workspaces via .gitignore Mapping**

AI agents frequently require scratchpads—temporary environments to store intermediate reasoning paths, partial code compilations, or localized error logs. Saving these highly volatile, temporary files to the global CRDT log or the permanent Git history would cause massive repository bloat and trigger continuous, unnecessary network synchronizations.  
Quarry ingeniously resolves this by binding the FUSE logic directly to standard .gitignore patterns.34 When the Quarry daemon starts, it parses the repository's .gitignore file. If an agent creates a file or directory that matches an ignored pattern (for example, a directory named agent\_ephemeral/ appended with a trailing slash in the .gitignore) 36, the FUSE daemon dynamically alters its storage strategy.  
Instead of routing writes in the ignored directory to the CRDT append-only session logs, the FUSE daemon routes them entirely to a high-speed, volatile tmpfs RAM disk.31 The agent experiences no difference—it can still grep, read, and write to these ephemeral files natively.37 However, because the FUSE layer intercepts the operation before it hits the CRDT engine, these files vanish when the system reboots and are entirely invisible to the synchronization protocols, perfectly balancing agent autonomy with repository cleanliness.

## **4\. Bi-Directional Synchronization: Git and Sovereign Collaboration**

While the CRDT engine handles real-time, microsecond-level synchronization, Git remains the undisputed industry standard for long-term historical provenance, branching, and ecosystem integration. Quarry acts as a continuous translation bridge between the fluid, operational state of the Yjs CRDTs and the rigid, commit-based architecture of a Git repository.

### **4.1 Automated Headless Git Integration**

The synchronization between Quarry and Git is entirely automated, shielding human users and agents from the complexities of manual version control management. Drawing on the mechanics of tools like GitFS, Quarry monitors the state of the CRDT engine.4 When an agent finishes a block of work, or a human user hits the "publish" button on a collaborative draft, the Quarry headless daemon triggers a batching sequence.4  
The daemon serializes the current state of the Yjs documents into standard plain-text files.23 It then executes headless Git commands (git add., git commit \-m "Auto-commit from Quarry") in the background.39 Because this process runs headlessly, the system suppresses interactive GUI prompts like gnome-ssh-askpass by unsetting environment variables (SSH\_ASKPASS) to prevent the daemon from hanging indefinitely.39

### **4.2 The Two-Way Synchronization Bridge**

Achieving one-way sync (saving CRDT state to Git) is trivial; the true architectural complexity lies in two-way synchronization, where external commits pushed to the remote Git server must be reflected back into the real-time CRDT environment without overwriting local agent operations.  
Quarry resolves this by maintaining a tripartite repository structure locally: a Production state (the CRDT view), an Upstream Git mirror, and an Intermediate integration repository (I-repo).40 The synchronization daemon operates on a continuous polling or webhook-driven loop. When an external change is detected on the remote server, the daemon fetches the commit into the Upstream mirror. It then rebases the Intermediate repository with both the Upstream mirror and the current Production CRDT state.40  
If the daemon identifies textual differences originating from the external Git commit, it calculates the delta and injects those changes into the Yjs instance as simulated operations. The CRDT engine's mathematical properties (commutativity and associativity) guarantee that these injected external changes cleanly merge with whatever the agents or human users are currently typing, effectively achieving a flawless two-way sync.1

### **4.3 Peer-to-Peer Sovereign Collaboration**

By merging Git with CRDTs, Quarry positions itself within the emerging paradigm of decentralized, sovereign code collaboration. Traditional code forges (like GitHub or GitLab) centralize repository control, creating dependencies and potential single points of failure.41  
Quarry draws architectural inspiration from Radicle, an open-source, peer-to-peer code collaboration stack built entirely on Git.41 Radicle utilizes a gossip-based networking layer and a repository identity system powered by CRDTs built on top of Git.43 By adopting similar decentralized peer discovery protocols 45, a cluster of Quarry instances can synchronize their Yjs document updates directly with one another over local networks or WebRTC, bypassing central servers entirely.18 This ensures that even in completely air-gapped or offline environments, human operators and AI agents can continue to collaborate, with Quarry acting as a serverless, peer-to-peer data mesh.44

## **5\. The Cognitive Layer: Advanced Agent Memory Systems**

File systems and Git repositories manage project state, but true autonomous capability requires deep contextual awareness. AI agents suffer from stringent context-window limitations; they cannot hold an entire repository or the complete history of human preferences in memory simultaneously.47 Quarry transcends basic storage by serving as the highly optimized foundational layer for persistent AI memory systems, allowing solutions like TrueMemory, AgentMemory, and Supermemory to operate seamlessly on top of the collaborative dataset.8

### **5.1 Memory Storage Architectures**

Advanced memory systems rely heavily on SQLite databases integrated with vector indexing extensions to store and query cognitive data.49 Because Quarry projects a stable file system and utilizes a CRDT-over-FS backend, these memory databases can be stored directly within the Quarry directory. This means an agent's SQLite memory files (e.g., ./quarry/.memory/state\_store.db) are continuously replicated and synchronized across all user devices via the Yjs backend, ensuring the agent's "brain" is omnipresent.50

| Memory Architecture | Key Features | Evaluation Performance | Integration Synergy with Quarry | Reference |
| :---- | :---- | :---- | :---- | :---- |
| **TrueMemory** | 6-layer neuroscience-inspired retrieval pipeline; advanced Encoding Gate for noise filtration. | 93.0% on LoCoMo benchmark; SOTA on BEAM-1M. | Runs entirely on a single SQLite file, perfect for FUSE integration and CRDT synchronization. | 8 |
| **AgentMemory** | 4-tier memory consolidation (Working, Episodic, Semantic, Procedural); BM25 \+ Vector \+ Graph search. | 95.2% on LongMemEval benchmark. | Exposes a Model Context Protocol (MCP) server directly to agents, bypassing manual API calls. | 9 |
| **Supermemory** | Hybrid conversational memory utilizing SQLite, full-text search, and explicit user profiles. | Highly scalable memory API. | Provides open-source plugins for existing agents like Claude Code and OpenClaw. | 48 |

### **5.2 The Ingestion Pipeline and Encoding Gate**

A critical failure point of primitive agent memory is the indiscriminate storage of all conversational text, which leads to bloated vector databases filled with noise (e.g., "Hello," "Let me think about that").8 Quarry recommends implementing the ingestion logic pioneered by TrueMemory.  
Before any text string or agent observation is written to the SQLite database via the FUSE hook, it must pass through an "Encoding Gate".8 This gate utilizes a three-signal filter:

1. **Compression Novelty**: A mathematical measurement utilizing gzip-based information gain to determine if the incoming fact adds genuinely new data relative to the existing database.8  
2. **Speech-Act Salience**: A logistic scorer that filters out conversational filler and retains actionable statements.8  
3. **Embedding Pair-Diff**: Calculates the embedding divergence between the new message and existing memories on the same topic, specifically designed to catch when a user updates a preference.8

If a fact passes this gate (scoring above a defined threshold), an LLM Extractor parses it into atomic facts and categorizes it (e.g., *technical*, *preference*, *decision*) before it is finalized in the database.8

### **5.3 Hierarchical Consolidation and Retrieval**

Once ingested, Quarry leverages AgentMemory's four-tier consolidation framework to structure the data hierarchically: Working Memory (raw, short-term observations), Episodic Memory (compressed session summaries), Semantic Memory (extracted facts and codebase patterns), and Procedural Memory (workflows and execution patterns).9  
When an agent executing within Quarry needs to answer a query or understand a codebase, it triggers a sophisticated 6-layer retrieval pipeline.8 The pipeline bypasses the slow limitations of simple vector search by first executing a broad keyword search using SQLite's FTS5 (Full-Text Search) engine with temporal filtering.8 It then executes a dense vector search utilizing high-efficiency embedding models (such as Model2Vec potion-base-8M for CPU-only Edge tiers, or Qwen3-Embedding-0.6B for Base tiers).8  
The results from the FTS5 and vector streams are merged using Reciprocal Rank Fusion (RRF), passed through a salience noise filter, and finally reranked using a cross-encoder (like MiniLM-L-6-v2 or gte-reranker-modernbert) to ensure absolute precision before being injected into the agent's context window.8 Because Quarry handles the underlying synchronization of these SQLite files, this complex retrieval architecture can operate consistently regardless of which physical machine the agent is running on.

## **6\. The Human Interface: Collaborative Drafts, PlateJS, and Provenance**

While the FUSE daemon and SQLite memory pipelines cater to the programmatic needs of AI agents, human operators require an intuitive, highly responsive Graphical User Interface (GUI). Quarry provides a web-based, rich-text environment dedicated to real-time human-agent co-authoring, redlining, and review.

### **6.1 WYSIWYG Editing with PlateJS**

The Quarry visual interface is constructed using PlateJS, a sophisticated React framework built on top of Slate.js.55 PlateJS was strategically selected over alternatives like Tiptap due to its seamless React integration and its highly modular, block-based plugin architecture.56

| Feature Comparison | PlateJS | Tiptap | Implication for Quarry | Reference |
| :---- | :---- | :---- | :---- | :---- |
| **Underlying Engine** | Slate.js | ProseMirror | PlateJS offers deeper React coupling, allowing complex UI components (like overlapping comment bubbles) to render natively. | 56 |
| **Framework Agnostic** | React-only | React, Vue, Vanilla | Quarry's human interface is strictly a React application, maximizing PlateJS's ecosystem advantages. | 56 |
| **Extensibility** | High (Plugin array) | High (Extensions) | PlateJS's array of out-of-the-box plugins (Headings, Blockquotes, Media) accelerates initial deployment. | 56 |

Integrating a real-time CRDT engine with a React-based text editor presents severe performance challenges. If the React component re-renders every time the Yjs engine receives a microsecond update from a concurrent agent, the browser's Document Object Model (DOM) will lock up. Quarry circumvents this by strictly adhering to PlateJS optimization patterns. Developers must utilize useEditorRef for callbacks, which maintains a mutable reference to the editor without triggering re-renders.10 When the UI must update, it utilizes useEditorSelector to subscribe only to specific, highly localized state changes, ensuring massive documents remain buttery smooth even when multiple agents are injecting code simultaneously.10

### **6.2 Collaborative Drafts and the .draft Lifecycle**

A critical requirement of the Quarry architecture is the "Collaborative Draft" workflow. Humans rarely want AI agents publishing raw, unreviewed code or documentation directly to the primary branch. When a human reviews an agent's output, they must be able to leave inline critiques, redlines, and comments.  
This is facilitated through the integration of specific PlateJS plugins:

* **CommentKit & DiscussionKit**: These plugins allow users to highlight text and attach floating conversational threads.58 These comments are stored as metadata markers within the Yjs text array, enabling multiple users to place overlapping annotations on the same segment of text seamlessly.55  
* **Suggestion Plugin**: This plugin handles the visual redlining, marking proposed text additions in specific colors and text removals with strikethroughs, while maintaining the underlying CRDT state.60

When a human initiates a collaborative review, the interface generates a temporary file appended with a .draft extension within the Quarry virtual file system. Agents are programmed to monitor the directory for .draft files. The agent reads the .draft file, parses the CRDT-encoded comments and redlines left by the human, and executes the necessary revisions. The draft remains in this iterative loop until the human user clicks "Save." Upon this trigger, the PlateJS interface resolves the suggestion markers, the Quarry daemon removes the .draft extension, and the finalized document is batched for Git synchronization.60

### **6.3 Provenance Tracking via Agent Bridges**

In an ecosystem where humans and multiple AI personas rapidly alter the same document, tracking authorship is paramount. Drawing inspiration from Every's "Proof" editor, Quarry implements rigorous provenance tracking.11  
The visual interface uses colored rails on the margins of the text editor to denote authorship—for example, a green rail indicates human-authored text, while a purple rail indicates AI-authored text.11 To achieve this, Quarry relies on an HTTP Agent Bridge, conceptually similar to the open-source proof-sdk.12  
When an agent interacts with Quarry via the REST API rather than the FUSE mount, it must authenticate using an X-Agent-Id HTTP header (e.g., X-Agent-Id: ai:codex or ai:claude).63 The headless Yjs synchronization server intercepts this header and tags the incoming CRDT binary update with the specific agent's clientID and transactionOrigin.23 The PlateJS frontend decodes this metadata and renders the colored provenance rails dynamically, providing absolute transparency regarding who—or what—wrote a specific line of code.11

## **7\. Handling Binary Data: PDFs and Immutable Synchronization**

Quarry's collaborative capabilities extend beyond plain text and markdown to include binary file formats, specifically Portable Document Format (PDF) files. Synchronizing binary files using CRDTs is notoriously complex; if an agent and a human attempt to concurrently modify a raw binary stream, the CRDT will interleave the bytes, resulting in a corrupted, unreadable file.15  
To resolve this, Quarry adopts an immutable wrapper strategy for binary assets.65 The raw PDF file itself is never subjected to CRDT interleaving. Instead, it is treated as an immutable blob and saved either as a standard binary file via the FUSE integration or converted into a base64 string.66  
All collaborative comments, highlights, and critiques intended for the PDF are completely decoupled from the binary file.14 Using a client-side rendering library (such as PDF.js or PSPDFKit), the human user views the PDF in the browser. When they draw a highlight or add a comment, the interface extracts the spatial coordinates and textual data of the annotation. This annotation data is serialized into JSON and stored within a Y.Map linked to the PDF's unique document ID.67  
When another user or an agent opens the PDF, the browser renders the immutable base64 binary layer, and the Quarry frontend dynamically overlays the CRDT-synchronized annotations on top of the viewport.14 This architecture guarantees that AI agents and humans can collaborate on complex documents, redlining visual elements without ever risking binary corruption.

## **8\. API Ecosystem and Extensibility**

For Quarry to serve as the universal data layer, it must provide programmatic access points beyond the FUSE mount to support lightweight clients, webhooks, and modern AI orchestration frameworks.

### **8.1 Model Context Protocol (MCP)**

Quarry natively embeds a Model Context Protocol (MCP) server. MCP is a standardized architecture that bridges AI agents with external context providers.9 The Quarry MCP server exposes the entire file system and the underlying SQLite memory pipelines directly to any MCP-compliant client (such as Claude Desktop, Cursor, or Gemini CLI) without requiring the agent to navigate the FUSE directory manually.9  
Agents can access specific resources, such as quarry://status to read system health, or quarry://project/{name}/profile to load the compiled semantic profile of a codebase.9 Furthermore, the MCP server provides executable tools (skills) directly to the agent. An agent can call /recall to trigger the 6-layer hybrid search pipeline, or /remember to forcefully inject a critical insight into the permanent CRDT-backed SQLite store.9

### **8.2 Headless REST API**

In addition to MCP, Quarry operates a robust REST API, powered by a headless Yjs backend (similar to y-websocket-server or Hocuspocus).70 Because the headless server cannot rely on a browser's IndexedDB for persistence, it utilizes LevelDB or Redis adapters to persist the CRDT binary updates.46  
The REST API allows external services, CI/CD pipelines, or mobile applications to query the state of a document without initiating a full, persistent WebSocket connection.72 External services can inject text blocks, read hit/miss FUSE statistics, or trigger Git synchronization routines via standard HTTP GET and POST methods, ensuring Quarry integrates smoothly into larger enterprise architectures.72

## **9\. Comparative Landscape of Similar Resources**

To validate the architectural decisions within Quarry, it is necessary to examine the landscape of existing tools and protocols that attempt to solve similar challenges. Quarry synthesizes elements from all of these platforms into a single, cohesive substrate.

| Resource | Primary Function | Comparison to Quarry Architecture | Reference |
| :---- | :---- | :---- | :---- |
| **Radicle** | Sovereign peer-to-peer code collaboration. | Radicle builds CRDTs on top of Git for decentralized identity and issue tracking. Quarry expands this concept by applying CRDTs directly to the file contents themselves in real-time. | 41 |
| **GitFS** | Mounting Git repositories as FUSE file systems. | GitFS automates commits upon file saves. Quarry adopts this headless batching mechanism but interposes a real-time CRDT engine to prevent merge conflicts before they reach Git. | 3 |
| **Hocuspocus / Y-Sweet** | Managed Yjs WebSocket backends. | These services provide excellent centralized synchronization for web apps. Quarry operates its own headless engine to ensure local-first capabilities and deep FUSE integration. | 46 |
| **Proof SDK** | Open-source collaborative editor for agents. | Proof introduced provenance tracking and agent HTTP bridges. Quarry adopts the X-Agent-Id headers and PlateJS integration for its visual presentation layer. | 11 |
| **AgentMemory** | Persistent memory system for coding agents. | Operates purely as a background tool. Quarry ingests its 4-tier consolidation logic and embeds the SQLite databases directly into the CRDT-over-FS layer for multi-device sync. | 9 |

## **10\. Strategic Implementation Plan and Decision Gates**

The realization of the Quarry architecture requires a disciplined, phased rollout to manage the immense complexity of binding a CRDT engine to a kernel-level VFS and a React-based frontend.  
**Phase 1: Core Storage and CRDT-over-FS Synthesis**

* Initialize the Yjs engine and construct the headless WebSocket server utilizing LevelDB for local persistence.  
* Develop the modified BitCask directory structure, establishing the append-only Session Logs and the .loglist tracking mechanism.  
* Implement the FUSE daemon utilizing the cgofuse library, establishing the exact POSIX-to-CRDT mathematical mapping for the Read, Write, and Create system calls.

**Phase 2: Legacy Integration and Security Mapping**

* Construct the automated, headless Git synchronization daemon, implementing the I-repo strategy for bi-directional rebase execution.  
* Implement the .gitignore parsing logic within the FUSE daemon to enable the tmpfs RAM disk offloading for ephemeral agent scratchpads.  
* Establish the umask routing and Chmod3 configurations, utilizing the fuse.DELETE\_OK bitwise mask to protect published documents from agent-initiated deletion.

**Phase 3: Cognitive Substrate Deployment**

* Embed the SQLite memory databases into the CRDT layer.  
* Deploy the TrueMemory-inspired Encoding Gate, calibrating the weights for Compression Novelty and Speech-Act Salience.  
* Integrate the FUSE event hooks (e.g., PreToolUse, PostToolUse) directly into the memory ingestion pipeline.

**Phase 4: The Presentation Layer and Provenance**

* Construct the React application, integrating PlateJS with slate-yjs bindings.  
* Deploy the CommentKit, DiscussionKit, and Suggestion plugins to enable visual redlining.  
* Program the UI to recognize the .draft file extension, establishing the collaborative loop between human approval and agent execution.  
* Implement the HTTP Agent Bridge, enforcing X-Agent-Id tracking to render the colored provenance rails.

**Phase 5: Media and Protocol Expansion**

* Deploy the immutable base64 wrapper system for PDF storage, decoupling the binary data from the Yjs metadata annotation mapping.  
* Expose the Model Context Protocol (MCP) server endpoints.  
* Implement SAMBA network mapping as a fallback for agents operating in environments without FUSE capabilities.

### **10.1 Strategic Inquiries for the Project Principal**

To finalize the deployment parameters for the initial phases, several architectural variables require clarification from the project stakeholders:

1. **Git Upstream Conflict Strategy**: If an external Git commit severely conflicts with the current local CRDT state in a way that algorithmic interleaving produces uncompilable code, should the Quarry daemon prioritize the external Git state or forcefully overwrite the remote branch with the local CRDT state?  
2. **Memory Database Scale**: Will the SQLite memory databases be expected to store multi-gigabyte vector embeddings locally, or should the vector indexes be offloaded to an external provider (e.g., Pinecone or local Milvus) while Quarry only synchronizes the metadata?  
3. **Authentication Protocols**: For the REST API and the SAMBA network mapping, what authentication protocol is preferred (e.g., OAuth2, local PAM integration, or static bearer tokens) to secure the agent HTTP bridges?

## **11\. Conclusion**

The Quarry architecture represents a fundamental paradigm shift in how collaborative data is conceptualized, stored, and interacted with. By refusing to treat the file system as a static, isolated repository, Quarry transforms local storage into a dynamic, real-time projection of a mathematically secure CRDT matrix. This design effortlessly eliminates the friction between the programmatic needs of autonomous agents and the visual, iterative requirements of human operators.  
The integration of the FUSE daemon and SAMBA mapping guarantees that robust, legacy POSIX utilities and modern command-line agents can traverse and manipulate the data natively. Simultaneously, the Yjs CRDT engine ensures that thousands of concurrent multi-agent modifications are merged immutably without the threat of file locking or data corruption. The automated Git mapping preserves traditional version control sovereignty, while the embedded neuroscience-inspired memory pipelines ensure that AI agents possess deep, continuous context spanning the entire lifecycle of a project.  
Coupled with a highly optimized, PlateJS-driven interface that supports granular redlining, strict provenance tracking, and decoupled binary data collaboration, Quarry is positioned to serve as the definitive, unified data storage layer for the next generation of human-machine collaboration.

#### **Works cited**

1. Understanding CRDTs in Automerge \- GitHub Pages, accessed on May 21, 2026, [https://posit-dev.github.io/automerge-r/articles/crdt-concepts.html](https://posit-dev.github.io/automerge-r/articles/crdt-concepts.html)  
2. About CRDTs • Conflict-free Replicated Data Types, accessed on May 21, 2026, [https://crdt.tech/](https://crdt.tech/)  
3. Git \+ FUSE \+ Python \= GitFS \- LWN.net, accessed on May 21, 2026, [https://lwn.net/Articles/654075/](https://lwn.net/Articles/654075/)  
4. presslabs/gitfs: Version controlled file system · GitHub \- GitHub, accessed on May 21, 2026, [https://github.com/presslabs/gitfs](https://github.com/presslabs/gitfs)  
5. Building a Dynamic Filesystem with FUSE and Node.js: A Practical Approach, accessed on May 21, 2026, [https://dev.to/pinkiesky/building-a-dynamic-filesystem-with-fuse-and-nodejs-a-practical-approach-2ogo](https://dev.to/pinkiesky/building-a-dynamic-filesystem-with-fuse-and-nodejs-a-practical-approach-2ogo)  
6. cgofuse/fuse/fsop.go at master \- GitHub, accessed on May 21, 2026, [https://github.com/billziss-gh/cgofuse/blob/master/fuse/fsop.go](https://github.com/billziss-gh/cgofuse/blob/master/fuse/fsop.go)  
7. Using FUSE to map network statistics to directories \- DEV Community, accessed on May 21, 2026, [https://dev.to/r4dx/using-fuse-to-map-network-statistics-to-directories-3c1b](https://dev.to/r4dx/using-fuse-to-map-network-statistics-to-directories-3c1b)  
8. buildingjoshbetter/TrueMemory: A living memory system ... \- GitHub, accessed on May 21, 2026, [https://github.com/buildingjoshbetter/TrueMemory](https://github.com/buildingjoshbetter/TrueMemory)  
9. rohitg00/agentmemory: \#1 Persistent memory for AI coding ... \- GitHub, accessed on May 21, 2026, [https://github.com/rohitg00/agentmemory](https://github.com/rohitg00/agentmemory)  
10. Editor Methods \- Plate.js, accessed on May 21, 2026, [https://platejs.org/docs/editor-methods](https://platejs.org/docs/editor-methods)  
11. Introducing Proof \- Every, accessed on May 21, 2026, [https://every.to/on-every/introducing-proof](https://every.to/on-every/introducing-proof)  
12. Proof SDK: open-source collaborative editor, provenance model, and agent HTTP bridge · GitHub, accessed on May 21, 2026, [https://github.com/EveryInc/proof-sdk](https://github.com/EveryInc/proof-sdk)  
13. A Collaborative Editor \- Yjs Docs, accessed on May 21, 2026, [https://docs.yjs.dev/getting-started/a-collaborative-editor](https://docs.yjs.dev/getting-started/a-collaborative-editor)  
14. PDF Annotations as custom metadata \- DEVONthink \- DEVONtechnologies Community, accessed on May 21, 2026, [https://discourse.devontechnologies.com/t/pdf-annotations-as-custom-metadata/76788](https://discourse.devontechnologies.com/t/pdf-annotations-as-custom-metadata/76788)  
15. Git is obviously not a CRDT because merge conflicts have to be manually resolved... | Hacker News, accessed on May 21, 2026, [https://news.ycombinator.com/item?id=40791869](https://news.ycombinator.com/item?id=40791869)  
16. Conflict-free replicated data type \- Wikipedia, accessed on May 21, 2026, [https://en.wikipedia.org/wiki/Conflict-free\_replicated\_data\_type](https://en.wikipedia.org/wiki/Conflict-free_replicated_data_type)  
17. Code (Implementations) \- Conflict-free Replicated Data Types, accessed on May 21, 2026, [https://crdt.tech/implementations](https://crdt.tech/implementations)  
18. yjs/yjs: Shared data types for building collaborative software \- GitHub, accessed on May 21, 2026, [https://github.com/yjs/yjs](https://github.com/yjs/yjs)  
19. Yjs Docs: Introduction, accessed on May 21, 2026, [https://docs.yjs.dev/](https://docs.yjs.dev/)  
20. Automerge, accessed on May 21, 2026, [https://automerge.org/](https://automerge.org/)  
21. A JSON-like data structure (a CRDT) that can be modified concurrently by different users, and merged again automatically. : r/programming \- Reddit, accessed on May 21, 2026, [https://www.reddit.com/r/programming/comments/sxwjpg/a\_jsonlike\_data\_structure\_a\_crdt\_that\_can\_be/](https://www.reddit.com/r/programming/comments/sxwjpg/a_jsonlike_data_structure_a_crdt_that_can_be/)  
22. Yjs vs Loro (new CRDT lib) \- Show, accessed on May 21, 2026, [https://discuss.yjs.dev/t/yjs-vs-loro-new-crdt-lib/2567](https://discuss.yjs.dev/t/yjs-vs-loro-new-crdt-lib/2567)  
23. Document Updates | Yjs Docs, accessed on May 21, 2026, [https://docs.yjs.dev/api/document-updates](https://docs.yjs.dev/api/document-updates)  
24. 3timeslazy/crdt-over-fs: Experimenting with synchronization ... \- GitHub, accessed on May 21, 2026, [https://github.com/3timeslazy/crdt-over-fs](https://github.com/3timeslazy/crdt-over-fs)  
25. Conflict-free Database over Virtual File System \- Bartosz Sypytkowski, accessed on May 21, 2026, [https://www.bartoszsypytkowski.com/conflict-free-database-over-virtual-file-system/](https://www.bartoszsypytkowski.com/conflict-free-database-over-virtual-file-system/)  
26. fuse package \- github.com/winfsp/cgofuse/fuse \- Go Packages, accessed on May 21, 2026, [https://pkg.go.dev/github.com/winfsp/cgofuse/fuse](https://pkg.go.dev/github.com/winfsp/cgofuse/fuse)  
27. Releases · winfsp/cgofuse \- GitHub, accessed on May 21, 2026, [https://github.com/winfsp/cgofuse/releases](https://github.com/winfsp/cgofuse/releases)  
28. DFUSE: Strongly Consistent Write-Back Kernel Caching for Distributed Userspace File Systems \- arXiv, accessed on May 21, 2026, [https://arxiv.org/html/2503.18191v3](https://arxiv.org/html/2503.18191v3)  
29. Linux Fuse File System Performance Learning | by Xiaolong Jiang \- Medium, accessed on May 21, 2026, [https://medium.com/@xiaolongjiang/linux-fuse-file-system-performance-learning-efb23a1fb83f](https://medium.com/@xiaolongjiang/linux-fuse-file-system-performance-learning-efb23a1fb83f)  
30. Overview of caching in Cloud Storage FUSE \- Google Cloud Documentation, accessed on May 21, 2026, [https://docs.cloud.google.com/storage/docs/cloud-storage-fuse/caching](https://docs.cloud.google.com/storage/docs/cloud-storage-fuse/caching)  
31. File caching in Cloud Storage FUSE \- Google Cloud Documentation, accessed on May 21, 2026, [https://docs.cloud.google.com/storage/docs/cloud-storage-fuse/file-caching](https://docs.cloud.google.com/storage/docs/cloud-storage-fuse/file-caching)  
32. SyncFS is a Filesystem in Userspace (FUSE) that offers something between mounting a cloud storage system using FUSE while keeping all changes remotely, and syncing a Cloud drive locally. · GitHub, accessed on May 21, 2026, [https://github.com/kevina/syncfs](https://github.com/kevina/syncfs)  
33. Reading fuse mount and filesystem stats \- linux \- Server Fault, accessed on May 21, 2026, [https://serverfault.com/questions/1147643/reading-fuse-mount-and-filesystem-stats](https://serverfault.com/questions/1147643/reading-fuse-mount-and-filesystem-stats)  
34. .gitignore file \- ignoring files in Git | Atlassian Git Tutorial, accessed on May 21, 2026, [https://www.atlassian.com/git/tutorials/saving-changes/gitignore](https://www.atlassian.com/git/tutorials/saving-changes/gitignore)  
35. Ignoring files \- GitHub Docs, accessed on May 21, 2026, [https://docs.github.com/en/get-started/git-basics/ignoring-files](https://docs.github.com/en/get-started/git-basics/ignoring-files)  
36. How to Ignore Git Folders and Directories .gitignore \- YouTube, accessed on May 21, 2026, [https://www.youtube.com/watch?v=qSnjgEU6VwQ](https://www.youtube.com/watch?v=qSnjgEU6VwQ)  
37. git ignoring a directory, it's like it doesn't exist \- Stack Overflow, accessed on May 21, 2026, [https://stackoverflow.com/questions/10759034/git-ignoring-a-directory-its-like-it-doesnt-exist](https://stackoverflow.com/questions/10759034/git-ignoring-a-directory-its-like-it-doesnt-exist)  
38. Why my git bare repo is automatically ignoring a directory. \- Reddit, accessed on May 21, 2026, [https://www.reddit.com/r/git/comments/ra75rk/why\_my\_git\_bare\_repo\_is\_automatically\_ignoring\_a/](https://www.reddit.com/r/git/comments/ra75rk/why_my_git_bare_repo_is_automatically_ignoring_a/)  
39. using the git client on a headless linux server \- Stack Overflow, accessed on May 21, 2026, [https://stackoverflow.com/questions/16348688/using-the-git-client-on-a-headless-linux-server](https://stackoverflow.com/questions/16348688/using-the-git-client-on-a-headless-linux-server)  
40. 2-Way-Sync with GIT \- osha1 \- Medium, accessed on May 21, 2026, [https://ohadshai.medium.com/2-way-sync-with-git-2b48d1663e28](https://ohadshai.medium.com/2-way-sync-with-git-2b48d1663e28)  
41. Radicle: P2P, Censorship-Resistant Code Collaboration Based on Git \- CCC Event Blog, accessed on May 21, 2026, [https://events.ccc.de/congress/2025/hub/en/event/detail/radicle-p2p-censorship-resistant-code-collaboratio](https://events.ccc.de/congress/2025/hub/en/event/detail/radicle-p2p-censorship-resistant-code-collaboratio)  
42. Radicle \- GitHub, accessed on May 21, 2026, [https://github.com/radicle-dev](https://github.com/radicle-dev)  
43. How we built a gossip layer and CRDT on top of Git \- Alexis Sellier | GitMerge 2024, accessed on May 21, 2026, [https://www.youtube.com/watch?v=tsVa53SPIHc](https://www.youtube.com/watch?v=tsVa53SPIHc)  
44. Radicle: peer-to-peer collaboration with Git : r/opensource \- Reddit, accessed on May 21, 2026, [https://www.reddit.com/r/opensource/comments/1tgbwfs/radicle\_peertopeer\_collaboration\_with\_git/](https://www.reddit.com/r/opensource/comments/1tgbwfs/radicle_peertopeer_collaboration_with_git/)  
45. Radicle: Peer-to-Peer Code Collaboration \- FOSDEM 2026, accessed on May 21, 2026, [https://fosdem.org/2026/schedule/event/TMQZTP-radicle/](https://fosdem.org/2026/schedule/event/TMQZTP-radicle/)  
46. Yjs | Homepage, accessed on May 21, 2026, [https://yjs.dev/](https://yjs.dev/)  
47. Local perpetual memory MEM0 | Supermemory \- NVIDIA Developer Forums, accessed on May 21, 2026, [https://forums.developer.nvidia.com/t/local-perpetual-memory-mem0-supermemory/350499](https://forums.developer.nvidia.com/t/local-perpetual-memory-mem0-supermemory/350499)  
48. GitHub \- supermemoryai/supermemory: Memory engine and app that is extremely fast, scalable. The Memory API for the AI era., accessed on May 21, 2026, [https://github.com/supermemoryai/supermemory](https://github.com/supermemoryai/supermemory)  
49. supermemory · GitHub Topics, accessed on May 21, 2026, [https://github.com/topics/supermemory](https://github.com/topics/supermemory)  
50. agentmemory/AGENTS.md at main · rohitg00/agentmemory \- GitHub, accessed on May 21, 2026, [https://github.com/rohitg00/agentmemory/blob/main/AGENTS.md](https://github.com/rohitg00/agentmemory/blob/main/AGENTS.md)  
51. Storage Is Not Memory: A Retrieval-Centered Architecture for Agent Recall \- arXiv, accessed on May 21, 2026, [https://arxiv.org/html/2605.04897v1](https://arxiv.org/html/2605.04897v1)  
52. I built agentmemory — your AI coding agent now remembers everything across sessions (Claude Code, Cursor, Gemini CLI, any MCP client) : r/ChatGPT \- Reddit, accessed on May 21, 2026, [https://www.reddit.com/r/ChatGPT/comments/1sfr0jy/i\_built\_agentmemory\_your\_ai\_coding\_agent\_now/](https://www.reddit.com/r/ChatGPT/comments/1sfr0jy/i_built_agentmemory_your_ai_coding_agent_now/)  
53. supermemory \- GitHub, accessed on May 21, 2026, [https://github.com/supermemoryai](https://github.com/supermemoryai)  
54. How should agent memory work when the agent needs reviewed project knowledge? : r/OpenAI \- Reddit, accessed on May 21, 2026, [https://www.reddit.com/r/OpenAI/comments/1taxbo0/how\_should\_agent\_memory\_work\_when\_the\_agent\_needs/](https://www.reddit.com/r/OpenAI/comments/1taxbo0/how_should_agent_memory_work_when_the_agent_needs/)  
55. Plate.js, accessed on May 21, 2026, [https://platejs.org/](https://platejs.org/)  
56. Plate.js vs. Tiptap: Simplicity Meets Capability, accessed on May 21, 2026, [https://tiptap.dev/alternatives/plate-vs-tiptap](https://tiptap.dev/alternatives/plate-vs-tiptap)  
57. Plugins \- Plate.js, accessed on May 21, 2026, [https://platejs.org/docs/plugins](https://platejs.org/docs/plugins)  
58. How to Organize Comments Into Bubbles Like PlateJS Playground · udecode plate · Discussion \#4718 \- GitHub, accessed on May 21, 2026, [https://github.com/udecode/plate/discussions/4718](https://github.com/udecode/plate/discussions/4718)  
59. Comment \- Plate JS, accessed on May 21, 2026, [https://platejs.org/docs/comment](https://platejs.org/docs/comment)  
60. Suggestion \- Plate.js, accessed on May 21, 2026, [https://platejs.org/docs/suggestion](https://platejs.org/docs/suggestion)  
61. What is Proof? | Every Help Center, accessed on May 21, 2026, [https://help.every.to/en/articles/14291292-what-is-proof](https://help.every.to/en/articles/14291292-what-is-proof)  
62. We Made a Document Editor Where Humans and AI Work Side by Side \- Apple Podcasts, accessed on May 21, 2026, [https://podcasts.apple.com/us/podcast/we-made-a-document-editor-where-humans-and-ai-work/id1719789201?i=1000754671870](https://podcasts.apple.com/us/podcast/we-made-a-document-editor-where-humans-and-ai-work/id1719789201?i=1000754671870)  
63. Proof — The agent-first document editor, accessed on May 21, 2026, [https://www.proofeditor.ai/](https://www.proofeditor.ai/)  
64. Document branches like git branches? \- Yjs Community, accessed on May 21, 2026, [https://discuss.yjs.dev/t/document-branches-like-git-branches/697](https://discuss.yjs.dev/t/document-branches-like-git-branches/697)  
65. neftaly/y-immutable: Typed immutable wrapper for Y.js \- GitHub, accessed on May 21, 2026, [https://github.com/neftaly/y-immutable](https://github.com/neftaly/y-immutable)  
66. How can I get base 64 or binary data from pdf after adding annotations? \- Stack Overflow, accessed on May 21, 2026, [https://stackoverflow.com/questions/75233912/how-can-i-get-base-64-or-binary-data-from-pdf-after-adding-annotations](https://stackoverflow.com/questions/75233912/how-can-i-get-base-64-or-binary-data-from-pdf-after-adding-annotations)  
67. Use YJS as main data structure \- Yjs Community, accessed on May 21, 2026, [https://discuss.yjs.dev/t/use-yjs-as-main-data-structure/463](https://discuss.yjs.dev/t/use-yjs-as-main-data-structure/463)  
68. Binding Yjs with PSPDFKit · Issue \#678 \- GitHub, accessed on May 21, 2026, [https://github.com/yjs/yjs/issues/678](https://github.com/yjs/yjs/issues/678)  
69. Display metadata associated with annotations with PDF and Javascript \- Stack Overflow, accessed on May 21, 2026, [https://stackoverflow.com/questions/74498260/display-metadata-associated-with-annotations-with-pdf-and-javascript](https://stackoverflow.com/questions/74498260/display-metadata-associated-with-annotations-with-pdf-and-javascript)  
70. yjs/y-websocket-server \- GitHub, accessed on May 21, 2026, [https://github.com/yjs/y-websocket-server](https://github.com/yjs/y-websocket-server)  
71. How to implement data persistence on the server side \- Yjs Community, accessed on May 21, 2026, [https://discuss.yjs.dev/t/how-to-implement-data-persistence-on-the-server-side/259](https://discuss.yjs.dev/t/how-to-implement-data-persistence-on-the-server-side/259)  
72. Stateless server broadcasting implementation (in Go) \- Yjs Community, accessed on May 21, 2026, [https://discuss.yjs.dev/t/stateless-server-broadcasting-implementation-in-go/393](https://discuss.yjs.dev/t/stateless-server-broadcasting-implementation-in-go/393)  
73. Yjs Fundamentals — Part 2: Sync & Awareness | by Dovetail Engineering \- Medium, accessed on May 21, 2026, [https://medium.com/dovetail-engineering/yjs-fundamentals-part-2-sync-awareness-73b8fabc2233](https://medium.com/dovetail-engineering/yjs-fundamentals-part-2-sync-awareness-73b8fabc2233)