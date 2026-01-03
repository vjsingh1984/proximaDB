import React, { useEffect, useRef, useState, useCallback } from 'react';
import cytoscape, { Core, NodeSingular, EdgeSingular, ElementDefinition } from 'cytoscape';
import './GraphVisualizationTab.css';

interface GraphNode {
  id: string;
  label: string;
  properties: Record<string, unknown>;
}

interface GraphEdge {
  id: string;
  source: string;
  target: string;
  type: string;
  weight?: number;
  properties?: Record<string, unknown>;
}

interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

interface LayoutOption {
  id: string;
  name: string;
  description: string;
}

const LAYOUT_OPTIONS: LayoutOption[] = [
  { id: 'cose', name: 'Force-Directed', description: 'Physics-based layout with node repulsion' },
  { id: 'circle', name: 'Circle', description: 'Nodes arranged in a circle' },
  { id: 'grid', name: 'Grid', description: 'Nodes arranged in a grid' },
  { id: 'concentric', name: 'Concentric', description: 'Nodes in concentric circles by degree' },
  { id: 'breadthfirst', name: 'Hierarchical', description: 'Tree-like layout from root nodes' },
  { id: 'random', name: 'Random', description: 'Random node positions' },
];

const GraphVisualizationTab: React.FC = () => {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);
  const [graphs, setGraphs] = useState<string[]>([]);
  const [selectedGraph, setSelectedGraph] = useState<string>('');
  const [graphData, setGraphData] = useState<GraphData | null>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedNode, setSelectedNode] = useState<GraphNode | null>(null);
  const [selectedEdge, setSelectedEdge] = useState<GraphEdge | null>(null);
  const [layout, setLayout] = useState<string>('cose');
  const [traversalDepth, setTraversalDepth] = useState<number>(2);
  const [startNodeId, setStartNodeId] = useState<string>('');
  const [nodeFilter, setNodeFilter] = useState<string>('');
  const [edgeTypeFilter, setEdgeTypeFilter] = useState<string>('');
  const [stats, setStats] = useState<{ nodes: number; edges: number }>({ nodes: 0, edges: 0 });

  // Fetch available graphs
  useEffect(() => {
    const fetchGraphs = async () => {
      try {
        const response = await fetch('/api/v1/graphs');
        if (response.ok) {
          const data = await response.json();
          setGraphs(data.graphs || ['knowledge', 'social', 'product_catalog']);
        } else {
          // Demo graphs for development
          setGraphs(['knowledge', 'social', 'product_catalog']);
        }
      } catch {
        // Demo graphs for development
        setGraphs(['knowledge', 'social', 'product_catalog']);
      }
    };
    fetchGraphs();
  }, []);

  // Initialize Cytoscape
  useEffect(() => {
    if (!containerRef.current) return;

    const cy = cytoscape({
      container: containerRef.current,
      style: [
        {
          selector: 'node',
          style: {
            'background-color': '#89b4fa',
            'label': 'data(label)',
            'color': '#cdd6f4',
            'text-valign': 'bottom',
            'text-halign': 'center',
            'font-size': '12px',
            'width': 40,
            'height': 40,
            'border-width': 2,
            'border-color': '#585b70',
            'text-margin-y': 6,
          },
        },
        {
          selector: 'node:selected',
          style: {
            'background-color': '#f38ba8',
            'border-color': '#f38ba8',
            'border-width': 3,
          },
        },
        {
          selector: 'node.highlighted',
          style: {
            'background-color': '#a6e3a1',
            'border-color': '#a6e3a1',
          },
        },
        {
          selector: 'edge',
          style: {
            'width': 2,
            'line-color': '#585b70',
            'target-arrow-color': '#585b70',
            'target-arrow-shape': 'triangle',
            'curve-style': 'bezier',
            'label': 'data(type)',
            'font-size': '10px',
            'color': '#a6adc8',
            'text-rotation': 'autorotate',
            'text-margin-y': -10,
          },
        },
        {
          selector: 'edge:selected',
          style: {
            'width': 3,
            'line-color': '#f38ba8',
            'target-arrow-color': '#f38ba8',
          },
        },
        {
          selector: 'edge.highlighted',
          style: {
            'width': 3,
            'line-color': '#a6e3a1',
            'target-arrow-color': '#a6e3a1',
          },
        },
      ],
      layout: { name: 'cose' },
      minZoom: 0.2,
      maxZoom: 3,
      wheelSensitivity: 0.3,
    });

    // Event handlers
    cy.on('tap', 'node', (event) => {
      const node = event.target as NodeSingular;
      setSelectedNode({
        id: node.id(),
        label: node.data('label'),
        properties: node.data('properties') || {},
      });
      setSelectedEdge(null);
    });

    cy.on('tap', 'edge', (event) => {
      const edge = event.target as EdgeSingular;
      setSelectedEdge({
        id: edge.id(),
        source: edge.source().id(),
        target: edge.target().id(),
        type: edge.data('type'),
        weight: edge.data('weight'),
        properties: edge.data('properties'),
      });
      setSelectedNode(null);
    });

    cy.on('tap', (event) => {
      if (event.target === cy) {
        setSelectedNode(null);
        setSelectedEdge(null);
      }
    });

    cyRef.current = cy;

    return () => {
      cy.destroy();
    };
  }, []);

  // Load graph data
  const loadGraph = useCallback(async () => {
    if (!selectedGraph) return;

    setLoading(true);
    setError(null);

    try {
      const params = new URLSearchParams({
        graph: selectedGraph,
        limit: '500',
      });

      if (startNodeId) {
        params.append('start_node', startNodeId);
        params.append('depth', traversalDepth.toString());
      }

      const response = await fetch(`/api/v1/graphs/${selectedGraph}/nodes?${params}`);

      if (!response.ok) {
        throw new Error(`Failed to load graph: ${response.status}`);
      }

      const data = await response.json();
      setGraphData(data);
    } catch (err) {
      // Generate demo data for development
      const demoData = generateDemoGraphData(selectedGraph);
      setGraphData(demoData);
      setError(null);
    } finally {
      setLoading(false);
    }
  }, [selectedGraph, startNodeId, traversalDepth]);

  // Generate demo data for development/testing
  const generateDemoGraphData = (graphName: string): GraphData => {
    const nodes: GraphNode[] = [];
    const edges: GraphEdge[] = [];

    const nodeCount = 20;
    const labels = graphName === 'knowledge'
      ? ['Entity', 'Concept', 'Document', 'Person']
      : graphName === 'social'
      ? ['User', 'Post', 'Comment', 'Group']
      : ['Product', 'Category', 'Brand', 'Review'];

    const edgeTypes = graphName === 'knowledge'
      ? ['RELATES_TO', 'CONTAINS', 'REFERENCES', 'SIMILAR_TO']
      : graphName === 'social'
      ? ['FOLLOWS', 'LIKES', 'COMMENTS', 'BELONGS_TO']
      : ['HAS_CATEGORY', 'MADE_BY', 'REVIEWED', 'SIMILAR_TO'];

    for (let i = 0; i < nodeCount; i++) {
      nodes.push({
        id: `node_${i}`,
        label: labels[Math.floor(Math.random() * labels.length)],
        properties: {
          name: `${labels[i % labels.length]} ${i}`,
          created_at: new Date().toISOString(),
          score: Math.random().toFixed(2),
        },
      });
    }

    const edgeCount = nodeCount * 1.5;
    for (let i = 0; i < edgeCount; i++) {
      const source = Math.floor(Math.random() * nodeCount);
      let target = Math.floor(Math.random() * nodeCount);
      while (target === source) {
        target = Math.floor(Math.random() * nodeCount);
      }

      edges.push({
        id: `edge_${i}`,
        source: `node_${source}`,
        target: `node_${target}`,
        type: edgeTypes[Math.floor(Math.random() * edgeTypes.length)],
        weight: parseFloat((Math.random()).toFixed(2)),
      });
    }

    return { nodes, edges };
  };

  // Update Cytoscape with graph data
  useEffect(() => {
    if (!cyRef.current || !graphData) return;

    const cy = cyRef.current;
    cy.elements().remove();

    // Filter data
    let filteredNodes = graphData.nodes;
    let filteredEdges = graphData.edges;

    if (nodeFilter) {
      const filterLower = nodeFilter.toLowerCase();
      filteredNodes = graphData.nodes.filter(
        (n) => n.label.toLowerCase().includes(filterLower) ||
               n.id.toLowerCase().includes(filterLower)
      );
      const nodeIds = new Set(filteredNodes.map((n) => n.id));
      filteredEdges = graphData.edges.filter(
        (e) => nodeIds.has(e.source) && nodeIds.has(e.target)
      );
    }

    if (edgeTypeFilter) {
      const filterLower = edgeTypeFilter.toLowerCase();
      filteredEdges = filteredEdges.filter(
        (e) => e.type.toLowerCase().includes(filterLower)
      );
    }

    // Add elements
    const elements: ElementDefinition[] = [
      ...filteredNodes.map((node) => ({
        data: {
          id: node.id,
          label: node.label,
          properties: node.properties,
        },
      })),
      ...filteredEdges.map((edge) => ({
        data: {
          id: edge.id,
          source: edge.source,
          target: edge.target,
          type: edge.type,
          weight: edge.weight,
          properties: edge.properties,
        },
      })),
    ];

    cy.add(elements);
    cy.layout({ name: layout } as cytoscape.LayoutOptions).run();

    setStats({
      nodes: filteredNodes.length,
      edges: filteredEdges.length,
    });
  }, [graphData, layout, nodeFilter, edgeTypeFilter]);

  // Apply layout
  const applyLayout = useCallback((layoutName: string) => {
    if (!cyRef.current) return;
    setLayout(layoutName);
    cyRef.current.layout({ name: layoutName } as cytoscape.LayoutOptions).run();
  }, []);

  // Fit view
  const fitView = useCallback(() => {
    if (!cyRef.current) return;
    cyRef.current.fit();
  }, []);

  // Highlight neighbors of selected node
  const highlightNeighbors = useCallback(() => {
    if (!cyRef.current || !selectedNode) return;
    const cy = cyRef.current;
    cy.elements().removeClass('highlighted');
    const node = cy.getElementById(selectedNode.id);
    node.neighborhood().addClass('highlighted');
    node.addClass('highlighted');
  }, [selectedNode]);

  // Export graph as PNG
  const exportPng = useCallback(() => {
    if (!cyRef.current) return;
    const png = cyRef.current.png({ bg: '#1e1e2e', full: true, scale: 2 });
    const link = document.createElement('a');
    link.href = png;
    link.download = `${selectedGraph || 'graph'}_export.png`;
    link.click();
  }, [selectedGraph]);

  // Export graph as JSON
  const exportJson = useCallback(() => {
    if (!graphData) return;
    const json = JSON.stringify(graphData, null, 2);
    const blob = new Blob([json], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `${selectedGraph || 'graph'}_export.json`;
    link.click();
    URL.revokeObjectURL(url);
  }, [graphData, selectedGraph]);

  return (
    <div className="graph-viz-container">
      <div className="graph-viz-header">
        <h2>Graph Visualization</h2>
        <div className="graph-stats">
          <span className="stat-item">
            <span className="stat-label">Nodes:</span> {stats.nodes}
          </span>
          <span className="stat-item">
            <span className="stat-label">Edges:</span> {stats.edges}
          </span>
        </div>
      </div>

      <div className="graph-viz-content">
        {/* Controls Panel */}
        <div className="controls-panel">
          <div className="control-section">
            <h3>Graph Selection</h3>
            <select
              value={selectedGraph}
              onChange={(e) => setSelectedGraph(e.target.value)}
              className="control-select"
            >
              <option value="">Select a graph...</option>
              {graphs.map((graph) => (
                <option key={graph} value={graph}>
                  {graph}
                </option>
              ))}
            </select>
            <button
              className="control-btn load-btn"
              onClick={loadGraph}
              disabled={!selectedGraph || loading}
            >
              {loading ? 'Loading...' : 'Load Graph'}
            </button>
          </div>

          <div className="control-section">
            <h3>Traversal</h3>
            <input
              type="text"
              placeholder="Start node ID"
              value={startNodeId}
              onChange={(e) => setStartNodeId(e.target.value)}
              className="control-input"
            />
            <div className="depth-control">
              <label>Depth: {traversalDepth}</label>
              <input
                type="range"
                min="1"
                max="5"
                value={traversalDepth}
                onChange={(e) => setTraversalDepth(parseInt(e.target.value))}
                className="depth-slider"
              />
            </div>
          </div>

          <div className="control-section">
            <h3>Filters</h3>
            <input
              type="text"
              placeholder="Filter nodes by label/id"
              value={nodeFilter}
              onChange={(e) => setNodeFilter(e.target.value)}
              className="control-input"
            />
            <input
              type="text"
              placeholder="Filter by edge type"
              value={edgeTypeFilter}
              onChange={(e) => setEdgeTypeFilter(e.target.value)}
              className="control-input"
            />
          </div>

          <div className="control-section">
            <h3>Layout</h3>
            <div className="layout-options">
              {LAYOUT_OPTIONS.map((opt) => (
                <button
                  key={opt.id}
                  className={`layout-btn ${layout === opt.id ? 'active' : ''}`}
                  onClick={() => applyLayout(opt.id)}
                  title={opt.description}
                >
                  {opt.name}
                </button>
              ))}
            </div>
          </div>

          <div className="control-section">
            <h3>Actions</h3>
            <div className="action-buttons">
              <button className="control-btn" onClick={fitView}>
                Fit View
              </button>
              <button
                className="control-btn"
                onClick={highlightNeighbors}
                disabled={!selectedNode}
              >
                Show Neighbors
              </button>
              <button className="control-btn" onClick={exportPng}>
                Export PNG
              </button>
              <button className="control-btn" onClick={exportJson}>
                Export JSON
              </button>
            </div>
          </div>
        </div>

        {/* Graph Container */}
        <div className="graph-canvas-wrapper">
          {error && <div className="graph-error">{error}</div>}
          {loading && (
            <div className="graph-loading">
              <div className="loading-spinner"></div>
              <span>Loading graph data...</span>
            </div>
          )}
          <div ref={containerRef} className="graph-canvas" />

          {!graphData && !loading && (
            <div className="graph-placeholder">
              Select a graph and click "Load Graph" to visualize
            </div>
          )}
        </div>

        {/* Details Panel */}
        <div className="details-panel">
          <h3>Element Details</h3>
          {selectedNode ? (
            <div className="element-details">
              <div className="detail-header">
                <span className="detail-type">Node</span>
                <span className="detail-id">{selectedNode.id}</span>
              </div>
              <div className="detail-row">
                <span className="detail-label">Label:</span>
                <span className="detail-value">{selectedNode.label}</span>
              </div>
              <div className="detail-section">
                <h4>Properties</h4>
                {Object.entries(selectedNode.properties).map(([key, value]) => (
                  <div key={key} className="detail-row">
                    <span className="detail-label">{key}:</span>
                    <span className="detail-value">
                      {typeof value === 'object' ? JSON.stringify(value) : String(value)}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          ) : selectedEdge ? (
            <div className="element-details">
              <div className="detail-header">
                <span className="detail-type">Edge</span>
                <span className="detail-id">{selectedEdge.id}</span>
              </div>
              <div className="detail-row">
                <span className="detail-label">Type:</span>
                <span className="detail-value">{selectedEdge.type}</span>
              </div>
              <div className="detail-row">
                <span className="detail-label">Source:</span>
                <span className="detail-value">{selectedEdge.source}</span>
              </div>
              <div className="detail-row">
                <span className="detail-label">Target:</span>
                <span className="detail-value">{selectedEdge.target}</span>
              </div>
              {selectedEdge.weight !== undefined && (
                <div className="detail-row">
                  <span className="detail-label">Weight:</span>
                  <span className="detail-value">{selectedEdge.weight}</span>
                </div>
              )}
            </div>
          ) : (
            <div className="no-selection">
              Click on a node or edge to view details
            </div>
          )}
        </div>
      </div>
    </div>
  );
};

export default GraphVisualizationTab;
