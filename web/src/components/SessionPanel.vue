<!-- En-tête de session : choix de l'organisation, connexion, déconnexion. -->
<script setup lang="ts">
import { ref } from 'vue';
import { setOrg } from '../api';
import { val } from '../dom';
import type { Org, Session } from '../types';

defineProps<{ session: Session | null; orgs: Org[] }>();
const emit = defineEmits<{ changed: []; login: []; logout: [] }>();

const busy = ref(false);
const error = ref('');
const chooseOrg = (e: Event) => {
  const org = val(e);
  if (!org) return;
  busy.value = true;
  try { setOrg(org); emit('changed'); } catch (err) { error.value = (err as Error).message; } finally { busy.value = false; }
};
</script>

<template>
  <div class="session">
    <template v-if="session?.authenticated">
      <cc-select label="Organisation" :hiddenLabel="true" :inline="true" :options="orgs.map(o => ({ label: o.name, value: o.id }))" :value="session.org" placeholder="Choisir une organisation" @cc-select="chooseOrg" :disabled="busy"></cc-select>
      <cc-button v-if="session.own_tokens" :outlined="true" :hideText="true" :title="'Déconnexion' + (session.email ? ' de ' + session.email : '') + ' : oublie les jetons de cet onglet'" @cc-click="emit('logout')">Se déconnecter</cc-button>
    </template>
    <cc-button v-else :primary="true" title="Autorisation via Clever Cloud" @cc-click="emit('login')">Se connecter</cc-button>
    <cc-notice v-if="error" intent="danger" :message="error"></cc-notice>
  </div>
</template>
