package harness

import (
	"fmt"
	"slices"
	"strings"
	"sync"
)

type Registry struct {
	mu        sync.RWMutex
	factories map[ProviderID]Factory
}

func NewRegistry(factories ...Factory) (*Registry, error) {
	registry := &Registry{factories: make(map[ProviderID]Factory)}
	for _, factory := range factories {
		if err := registry.Register(factory); err != nil {
			return nil, err
		}
	}
	return registry, nil
}

func (r *Registry) Register(factory Factory) error {
	if factory == nil {
		return fmt.Errorf("register harness provider: factory is required")
	}
	provider := factory.Provider()
	if err := validateProvider(provider); err != nil {
		return err
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.factories[provider.ID]; exists {
		return fmt.Errorf("register harness provider %q: already registered", provider.ID)
	}
	r.factories[provider.ID] = factory
	return nil
}

func (r *Registry) Factory(id ProviderID) (Factory, error) {
	r.mu.RLock()
	factory := r.factories[id]
	r.mu.RUnlock()
	if factory == nil {
		return nil, &ProviderError{Provider: id, Operation: "resolve provider", Cause: ErrUnknownProvider}
	}
	return factory, nil
}

func (r *Registry) Providers() []Provider {
	r.mu.RLock()
	factories := make([]Factory, 0, len(r.factories))
	for _, factory := range r.factories {
		factories = append(factories, factory)
	}
	r.mu.RUnlock()
	providers := make([]Provider, 0, len(factories))
	for _, factory := range factories {
		providers = append(providers, factory.Provider())
	}
	slices.SortFunc(providers, func(left, right Provider) int { return strings.Compare(string(left.ID), string(right.ID)) })
	return providers
}

func validateProvider(provider Provider) error {
	if strings.TrimSpace(string(provider.ID)) == "" {
		return fmt.Errorf("register harness provider: ID is required")
	}
	if strings.TrimSpace(provider.DisplayName) == "" {
		return fmt.Errorf("register harness provider %q: display name is required", provider.ID)
	}
	if !provider.Capabilities.IdempotentSubmission && !provider.Capabilities.SubmissionLookup {
		return fmt.Errorf("register harness provider %q: safe submission recovery requires %s or %s", provider.ID, CapabilityIdempotentSubmission, CapabilitySubmissionLookup)
	}
	return nil
}
