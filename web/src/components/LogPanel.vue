<!-- Journal d'une opération (up, down, seed, logs) : lignes colorées,
     suivi collant, agrandissement, interruption. -->
<script setup lang="ts">
import { ref, watch } from 'vue';
import { renderLine } from '../ansi';

const props = defineProps<{ lines: string[]; running: boolean; title: string }>();
const emit = defineEmits<{ cancel: []; clear: [] }>();

const el = ref<HTMLElement | null>(null);
const big = ref(false);
// Suivi collant : on ne descend automatiquement que si l'utilisateur
// était déjà en bas ; sinon il garde sa position.
const atBottom = () => !el.value || el.value.scrollHeight - el.value.scrollTop - el.value.clientHeight < 40;
watch(() => props.lines.length, () => {
  const follow = atBottom();
  requestAnimationFrame(() => { if (el.value && follow) el.value.scrollTop = el.value.scrollHeight; });
}, { flush: 'pre' });
const toBottom = () => { if (el.value) el.value.scrollTop = el.value.scrollHeight; };
</script>

<template>
  <section :class="['logs', { big }]" v-if="lines.length || running">
    <header>
      <strong>{{ title || 'Journal' }}</strong>
      <cc-loader v-if="running" a11y-name="en cours"></cc-loader>
      <span class="dim">{{ lines.length }} lignes</span>
      <span class="grow"></span>
      <cc-button :outlined="true" title="Aller à la fin" @cc-click="toBottom">↓ Fin</cc-button>
      <cc-button :outlined="true" :title="big ? 'Réduire le journal' : 'Agrandir le journal'" @cc-click="big = !big">{{ big ? 'Réduire' : 'Agrandir' }}</cc-button>
      <cc-button v-if="running" :outlined="true" :danger="true" title="Interrompt l'opération en cours" @cc-click="emit('cancel')">Arrêter</cc-button>
      <cc-button v-else :outlined="true" title="Vide le journal" @cc-click="emit('clear')">Effacer</cc-button>
    </header>
    <pre ref="el"><div v-for="(l, i) in lines" :key="i" v-html="renderLine(l)"></div></pre>
  </section>
</template>
