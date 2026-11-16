<script>
  import MdNode from './MdNode.svelte';
  // Renders the inert tree from markdown.js: only text nodes and a fixed set of tags.
  let { node } = $props();
  const level = $derived(Math.min(6, Math.max(1, node.level || 1)));
</script>

{#if node.t === 'text'}{node.v}{:else if node.t === 'br'}<br>{:else if node.t === 'code'}<code>{node.v}</code>{:else if node.t === 'pre'}<pre><code>{node.v}</code></pre>{:else if node.t === 'a'}<a href={node.href} rel="noopener noreferrer nofollow" target="_blank">{#each node.children as child}<MdNode node={child} />{/each}</a>{:else if node.t === 'h'}<svelte:element this={'h' + level}>{#each node.children as child}<MdNode node={child} />{/each}</svelte:element>{:else if node.t === 'p'}<p>{#each node.children as child}<MdNode node={child} />{/each}</p>{:else if node.t === 'ul'}<ul>{#each node.children as child}<MdNode node={child} />{/each}</ul>{:else if node.t === 'ol'}<ol>{#each node.children as child}<MdNode node={child} />{/each}</ol>{:else if node.t === 'li'}<li>{#each node.children as child}<MdNode node={child} />{/each}</li>{:else if node.t === 'strong'}<strong>{#each node.children as child}<MdNode node={child} />{/each}</strong>{:else if node.t === 'em'}<em>{#each node.children as child}<MdNode node={child} />{/each}</em>{/if}
