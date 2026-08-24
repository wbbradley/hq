package harnessbridge

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync"

	"github.com/wbbradley/hq/internal/harness"
)

type requestPump struct {
	session    harness.Session
	questioner *questioner
	done       chan struct{}
	failed     chan struct{}
	errMu      sync.Mutex
	err        error
	failOnce   sync.Once
	wait       sync.WaitGroup
}

func startRequestPump(ctx context.Context, instance harness.Instance, questioner *questioner) *requestPump {
	pump := &requestPump{session: instance.Session(), questioner: questioner, done: make(chan struct{}), failed: make(chan struct{})}
	go pump.run(ctx, instance.Requests())
	return pump
}

func (p *requestPump) run(ctx context.Context, requests <-chan harness.Request) {
	defer close(p.done)
	for {
		select {
		case <-ctx.Done():
			p.wait.Wait()
			return
		case request, open := <-requests:
			if !open {
				p.wait.Wait()
				return
			}
			p.wait.Add(1)
			go func() {
				defer p.wait.Done()
				p.handle(ctx, request)
			}()
		}
	}
}

func (p *requestPump) handle(ctx context.Context, request harness.Request) {
	response, err := p.answer(ctx, request)
	if err != nil {
		response = harness.CancelResponse{Reason: err.Error()}
	}
	respondErr := p.session.Respond(ctx, harness.Response{RequestID: request.ID, Payload: response})
	if respondErr != nil && !errors.Is(respondErr, context.Canceled) && !errors.Is(respondErr, harness.ErrInstanceStopped) && !errors.Is(respondErr, harness.ErrRequestCompleted) {
		p.fail(fmt.Errorf("respond to harness request %s: %w", request.ID, respondErr))
	}
}

func (p *requestPump) answer(ctx context.Context, request harness.Request) (harness.ResponsePayload, error) {
	correlation := requestCorrelation{sessionID: string(request.Session.ID), operationID: string(request.Operation), itemID: request.ItemID, requestID: string(request.ID)}
	switch payload := request.Payload.(type) {
	case harness.QuestionSetRequest:
		for _, question := range payload.Questions {
			if question.Secret {
				_ = p.questioner.notice(context.Background(), "Sensitive input request rejected", "The harness requested a confidential answer. HQ stores message content, so it did not display or persist any request fields.", correlation)
				return nil, errors.New("HQ has no non-persistent secret input channel")
			}
		}
		pending := make([]*pendingQuestion, 0, len(payload.Questions))
		for _, question := range payload.Questions {
			spec := questionSpec{body: question.Prompt, details: questionDetails(question), correlation: correlation}
			published, err := p.questioner.publish(ctx, spec)
			if err != nil {
				for _, candidate := range pending {
					p.questioner.cancel(candidate)
				}
				return nil, err
			}
			pending = append(pending, published)
		}
		answers := make([]string, 0, len(payload.Questions))
		for index, question := range payload.Questions {
			value, err := p.awaitQuestion(ctx, pending[index], question)
			if err != nil {
				for _, candidate := range pending[index+1:] {
					p.questioner.cancel(candidate)
				}
				return nil, err
			}
			answers = append(answers, value)
		}
		return harness.AnswerResponse{Answers: answers}, nil
	case harness.QuestionRequest:
		if payload.Secret {
			_ = p.questioner.notice(context.Background(), "Sensitive input request rejected", "The harness requested a confidential answer. HQ stores message content, so it did not display or persist any request fields.", correlation)
			return nil, errors.New("HQ has no non-persistent secret input channel")
		}
		question := harness.Question{Prompt: payload.Prompt, Options: payload.Options, AllowOther: payload.AllowOther}
		value, err := p.questioner.ask(ctx, questionSpec{body: payload.Prompt, details: questionDetails(question), correlation: correlation}, questionValidator(question))
		if err != nil {
			return nil, err
		}
		return harness.AnswerResponse{Answers: []string{value.(harness.AnswerResponse).Answers[0]}}, nil
	case harness.ApprovalRequest:
		body := approvalBody(p.questioner.terms.ProviderName, payload.Kind)
		details := strings.TrimSpace(payload.Summary) + "\n\nLegal replies: " + strings.Join(payload.Choices, ", ")
		if payload.Persistent {
			details += "\nSome approval choices persist for this session."
		}
		return p.questioner.ask(ctx, questionSpec{body: body, details: details, correlation: correlation}, exactDecision(payload.Choices))
	case harness.StructuredQuestionRequest:
		details := "Schema:\n" + string(payload.Schema) + "\n\nLegal replies: accept {\"field\":\"value\"}, decline, cancel"
		return p.questioner.ask(ctx, questionSpec{body: payload.Prompt, details: details, correlation: correlation}, structuredAnswer)
	default:
		_ = p.questioner.notice(context.Background(), "Unsupported harness request", fmt.Sprintf("HQ cannot safely handle request payload %T.", request.Payload), correlation)
		return nil, fmt.Errorf("unsupported harness request payload %T", request.Payload)
	}
}

func (p *requestPump) awaitQuestion(ctx context.Context, pending *pendingQuestion, question harness.Question) (string, error) {
	value, err := p.questioner.await(ctx, pending)
	if err != nil {
		return "", err
	}
	answer := strings.TrimSpace(value.message.Body)
	validated, validationErr := questionValidator(question)(answer)
	if validationErr == nil {
		if err := value.complete(context.Background()); err != nil {
			_ = value.release(context.Background())
			return "", err
		}
		return validated.(harness.AnswerResponse).Answers[0], nil
	}
	if err := value.complete(context.Background()); err != nil {
		return "", err
	}
	replacement, err := p.questioner.ask(ctx, questionSpec{body: "Invalid reply; please answer again: " + question.Prompt, details: "Validation error: " + validationErr.Error() + "\n\n" + questionDetails(question), correlation: pending.spec.correlation}, questionValidator(question))
	if err != nil {
		return "", err
	}
	return replacement.(harness.AnswerResponse).Answers[0], nil
}

func questionValidator(question harness.Question) answerValidator {
	return func(answer string) (harness.ResponsePayload, error) {
		if answer == "" {
			return nil, errors.New("answer must not be empty")
		}
		for _, option := range question.Options {
			if answer == option.Label {
				return harness.AnswerResponse{Answers: []string{answer}}, nil
			}
		}
		if question.AllowOther || len(question.Options) == 0 {
			return harness.AnswerResponse{Answers: []string{answer}}, nil
		}
		return nil, errors.New("answer must exactly match one listed option")
	}
}

func questionDetails(question harness.Question) string {
	var details strings.Builder
	fmt.Fprintf(&details, "Question ID: %s\nLabel: %s\nFree-form answer allowed: %t", question.ID, question.Header, question.AllowOther || len(question.Options) == 0)
	if len(question.Options) > 0 {
		details.WriteString("\nOptions:")
		for _, option := range question.Options {
			fmt.Fprintf(&details, "\n- %s — %s", option.Label, option.Description)
		}
	}
	return details.String()
}

func approvalBody(provider, kind string) string {
	switch kind {
	case "command":
		return provider + " requests command approval"
	case "file-change":
		return provider + " requests approval for file changes"
	case "permissions":
		return provider + " requests additional permissions"
	case "elicitation-url":
		return provider + " requests external interaction"
	default:
		return provider + " requests approval"
	}
}

func (p *requestPump) fail(err error) {
	p.errMu.Lock()
	if p.err == nil {
		p.err = err
	}
	p.errMu.Unlock()
	p.failOnce.Do(func() { close(p.failed) })
}

func (p *requestPump) Err() error {
	p.errMu.Lock()
	defer p.errMu.Unlock()
	return p.err
}

func (p *requestPump) Done() <-chan struct{}   { return p.done }
func (p *requestPump) Failed() <-chan struct{} { return p.failed }
